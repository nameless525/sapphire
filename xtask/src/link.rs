//! `cargo xtask link <command>`: live link's desktop tool (spec §5.29, M1 A9), built for the host
//! and run with the workspace's cooked content (`--content target/content`), so `push` and
//! `watch` find the packs of the cook the device reads. Everything after `link` goes to the tool
//! as it is: `cargo xtask link status`, `cargo xtask link watch --addr 127.0.0.1:7811`.
//!
//! The tool is `sapphire_livelink`'s binary, run as a process as the pipeline is: xtask stays
//! small, and the tool links the engine's packs and A8's file watcher.

use std::process::Command;

use crate::platforms;
use crate::workspace::{self, CargoBuild};

const TOOL: &str = "sapphire_livelink";

pub fn run(args: Vec<String>) -> anyhow::Result<()> {
    let env = platforms::host_build_env()?;
    let tool = workspace::build_binary(
        &CargoBuild { package: TOOL, target: None, release: false, features: &[], env: &env },
        TOOL,
    )?;
    let mut command = Command::new(tool);
    command.arg("--content").arg(workspace::output_dir().join("content")).args(args);
    workspace::run(&mut command)
}
