//! `cargo xtask link <command>`: live link's desktop tool (spec §5.29, M1 A9), built for the host
//! and run with the workspace's cooked content (`--content target/content`), so `push` and
//! `watch` find the packs of the cook the device reads. Everything after `link` goes to the tool
//! as it is: `cargo xtask link status`, `cargo xtask link watch --addr 127.0.0.1:7811`.
//!
//! The tool is `sapphire_livelink`'s binary, run as a process as the pipeline is: xtask stays
//! small, and the tool links the engine's packs and A8's file watcher.
//!
//! It takes no `--app`. Nothing here differs per app: the tool reaches whichever app is at `--addr`
//! or dials `--listen`, `adb forward` and `iproxy` forward a port rather than a package, and every
//! app reads one cook. `status` says which app answered.

use std::process::Command;

use anyhow::ensure;

use crate::platforms;
use crate::workspace::{self, BuildProfile, CargoBuild};

const TOOL: &str = "sapphire_livelink";

pub fn run(args: Vec<String>) -> anyhow::Result<()> {
    check_args(&args)?;
    let env = platforms::host_build_env()?;
    let tool = workspace::build_binary(
        &CargoBuild { package: TOOL, target: None, profile: BuildProfile::Dev, features: &[], env: &env },
        TOOL,
    )?;
    let mut command = Command::new(tool);
    command.arg("--content").arg(workspace::output_dir().join("content")).args(args);
    workspace::run(&mut command)
}

/// `--app` is the one option of `run` and `bundle` someone would try here, so it gets an answer of its
/// own rather than the tool's usage text.
fn check_args(args: &[String]) -> anyhow::Result<()> {
    let mut options = args.iter().take_while(|arg| *arg != "--");
    ensure!(
        !options.any(|arg| arg == "--app" || arg.starts_with("--app=")),
        "`link` takes no --app: it reaches whichever app is at --addr, or dials --listen, and `status` \
         says which that is"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(line: &str) -> anyhow::Result<()> {
        check_args(&line.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
    }

    #[test]
    fn refuses_an_app_since_nothing_here_depends_on_one() {
        for line in ["--app game_app status", "status --app game_app", "push --app=game_app a.spack"] {
            assert!(check(line).unwrap_err().to_string().contains("takes no --app"), "{line}");
        }
        assert!(check("status --addr 127.0.0.1:7811").is_ok());
        assert!(check("push a.spack --apple").is_ok());
        assert!(check("push a.spack -- --app").is_ok(), "past `--` it's a file's name");
    }
}
