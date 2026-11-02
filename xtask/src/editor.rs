//! `cargo xtask editor [<file.moves.ron>] [editor args]`: the combat timeline editor (spec §5.28, M2 T3),
//! built for the host and pointed at the desktop cook, so a move's clips and the character are there to
//! preview. Everything after `editor` goes to the tool as it is.
//!
//! This opens a window, so it's for a person at the machine; agents use `--capture` and the crate's tests
//! (`docs/development-iteration.md`, "The timeline editor"). The cook's lock goes before it starts, so a
//! cook beside a running editor doesn't wait for it to close; the editor notices the new cook itself.

use std::process::Command;

use crate::content::{self, Cook};
use crate::platforms;
use crate::workspace::{self, BuildProfile, CargoBuild};

const TOOL: &str = "sapphire_editor";

pub fn run(args: Vec<String>) -> anyhow::Result<()> {
    let dir = content::cook(Cook::Desktop)?.dir.clone();
    let env = platforms::host_build_env()?;
    // The cooker's profile: optimized, without cross-crate LTO, and most of it built by that cook already.
    let tool = workspace::build_binary(
        &CargoBuild { package: TOOL, target: None, profile: BuildProfile::Tools, features: &[], env: &env },
        TOOL,
    )?;
    let mut command = Command::new(tool);
    command.arg("--content").arg(dir).args(args);
    workspace::run(&mut command)
}
