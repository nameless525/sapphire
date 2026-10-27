//! The host desktop (Windows, Linux or macOS): run the build in place against the cooked
//! content, or bundle it as a portable directory, the layout installers and Steam depots
//! start from (spec §2).

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use anyhow::Context as _;

use super::{App, Options, Platform, macos};
use crate::content::{self, Cook};
use crate::workspace;

use super::CONTENT_DIR_ENV;

/// Builds `target/desktop/<Name>/`: the executable with the desktop cook in `content/`
/// beside it, where the platform layer looks by default.
pub fn bundle(app: &App, options: &Options) -> anyhow::Result<PathBuf> {
    let content = content::cook(Cook::Desktop)?;
    let executable = super::executable(app, None, &build_env()?, options)?;
    let dir = super::bundle_dir(Platform::Desktop).join(&app.bundle_name);
    workspace::remove_dir_if_exists(&dir)?;
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let name = executable.file_name().context("the executable path has no file name")?;
    fs::copy(&executable, dir.join(name))
        .with_context(|| format!("failed to copy {} into the bundle", executable.display()))?;
    workspace::copy_dir(&content.dir, &dir.join("content"))?;
    eprintln!("bundled {}", super::display(&dir));
    Ok(dir)
}

/// Cooks for desktop, builds for the host and runs the executable in the foreground
/// against the cooked directory, without bundling.
pub fn run(app: &App, options: &Options, args: &[String]) -> anyhow::Result<()> {
    let (content, executable) = prepare(app, options)?;
    let mut command = Command::new(executable);
    command.args(args).env(CONTENT_DIR_ENV, &content.dir);
    // Held while the app runs, so no other xtask run recooks the content under it, unless the
    // build hot reloads: it remounts the packs `cargo xtask content` rewrites while it runs
    // (written under temporary names and renamed into place), so a recook must not wait for it.
    let _held = (!takes_recooks(options)).then_some(content);
    super::launch(&mut command, options)
}

/// Whether the build hot reloads its content (the sandbox's `dev` feature, spec §5.10), so a
/// recook while it runs is the point rather than a hazard.
fn takes_recooks(options: &Options) -> bool {
    options.features.iter().any(|feature| feature == "dev")
}

/// Cooks for desktop and builds for the host: what [`run`] launches.
pub fn prepare(app: &App, options: &Options) -> anyhow::Result<(content::Cooked, PathBuf)> {
    let content = content::cook(Cook::Desktop)?;
    let executable = super::executable(app, None, &build_env()?, options)?;
    Ok((content, executable))
}

/// Host-specific build environment: only macOS needs any (see [`macos::build_env`]).
pub fn build_env() -> anyhow::Result<Vec<(&'static str, OsString)>> {
    if std::env::consts::OS == "macos" { macos::build_env() } else { Ok(Vec::new()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_hot_reloading_builds_let_the_content_be_recooked_while_they_run() {
        let options = |features: &[&str]| Options {
            features: features.iter().map(|&feature| feature.to_owned()).collect(),
            ..Options::default()
        };
        assert!(takes_recooks(&options(&["dev"])));
        assert!(takes_recooks(&options(&["tracy", "dev"])));
        assert!(!takes_recooks(&options(&[])));
        assert!(!takes_recooks(&options(&["tracy"])));
    }
}
