//! The host desktop (Windows, Linux or macOS): run the build in place against the cooked
//! content, or bundle it as a portable directory, the layout installers and Steam depots
//! start from (spec §2).

use std::ffi::OsString;
use std::fs;
use std::process::Command;

use anyhow::Context as _;

use super::{App, Bundle, BundleClaim, HostExecutable, Options, Platform, macos};
use crate::content::{self, Cook};
use crate::workspace;

use super::CONTENT_DIR_ENV;

/// Builds `target/desktop/<Name>/`: the executable with the desktop cook in `content/`
/// beside it, where the platform layer looks by default.
pub fn bundle(app: &App, options: &Options) -> anyhow::Result<Bundle> {
    let claim = BundleClaim::take(Platform::Desktop)?;
    let content = content::cook(Cook::Desktop)?;
    let executable = HostExecutable::for_app(app, options)?;
    let executable = executable.path();
    let dir = claim.staging(&app.bundle_name)?;
    workspace::remove_dir_if_exists(&dir)?;
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let name = executable.file_name().context("the executable path has no file name")?;
    fs::copy(executable, dir.join(name))
        .with_context(|| format!("failed to copy {} into the bundle", executable.display()))?;
    workspace::copy_dir(&content.dir, &dir.join("content"))?;
    claim.publish(&dir)
}

/// Cooks for desktop, builds for the host and runs the executable in the foreground
/// against the cooked directory, without bundling.
pub fn run(app: &App, options: &Options, args: &[String]) -> anyhow::Result<()> {
    let (content, executable) = prepare(app, options)?;
    // A dry run's copy is gone by the time anyone reads the line.
    let mut command = Command::new(if options.dry_run { executable.built() } else { executable.path() });
    command.args(args).env(CONTENT_DIR_ENV, &content.dir);
    // Held while the app runs, so no other xtask run recooks the content under it, unless the
    // build hot reloads: it remounts the packs `cargo xtask content` rewrites while it runs
    // (written under temporary names and renamed into place), so a recook must not wait for it.
    let _held = (!super::takes_recooks(options)).then_some(content);
    super::launch(&mut command, options)
}

/// Cooks for desktop and builds for the host: what [`run`] launches.
pub fn prepare(app: &App, options: &Options) -> anyhow::Result<(content::Cooked, HostExecutable)> {
    let content = content::cook(Cook::Desktop)?;
    let executable = HostExecutable::for_app(app, options)?;
    Ok((content, executable))
}

/// Host-specific build environment: only macOS needs any (see [`macos::build_env`]).
pub fn build_env() -> anyhow::Result<Vec<(&'static str, OsString)>> {
    if std::env::consts::OS == "macos" { macos::build_env() } else { Ok(Vec::new()) }
}
