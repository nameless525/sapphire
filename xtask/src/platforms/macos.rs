//! macOS: bundle the host build as a signed `.app`, and run the bundled executable.
//! `--platform desktop` runs the unbundled build instead.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::Command;

use super::apple::{self, BundleLayout};
use super::{App, CONTENT_DIR_ENV, Options, Platform};
use crate::content::{self, Cook};
use crate::workspace;

/// Builds `target/macos/<Name>.app` with the desktop cook in `Contents/Resources/content`.
pub fn bundle(app: &App, options: &Options) -> anyhow::Result<PathBuf> {
    super::require_macos_host(Platform::MacOs)?;
    let content = content::cook(Cook::Desktop)?;
    let executable = super::executable(app, None, &build_env()?, options)?;
    let (template_path, template) = super::info_plist_template("macos")?;
    apple::verify_minimum_os(&executable, &template, "LSMinimumSystemVersion")?;
    let info_plist = apple::render_info_plist(&template, app)
        .map_err(|e| e.context(format!("in {}", template_path.display())))?;

    let layout =
        BundleLayout::macos(super::bundle_dir(Platform::MacOs).join(format!("{}.app", app.bundle_name)));
    apple::assemble(&layout, app, &info_plist, &executable, &content.dir)?;
    apple::codesign_ad_hoc(&layout.root)?;
    eprintln!("bundled {}", super::display(&layout.root));
    Ok(layout.root)
}

/// Bundles, then runs the bundled executable in the foreground, so the console stays
/// attached (see [`launch_command`] for where its content comes from).
pub fn run(app: &App, options: &Options, args: &[String]) -> anyhow::Result<()> {
    let bundle = bundle(app, options)?;
    let executable = BundleLayout::macos(bundle).executable_dir.join(&app.package);
    let inherited = std::env::var_os(CONTENT_DIR_ENV);
    super::launch(&mut launch_command(executable, args, options, inherited.as_deref()), options)
}

/// Runs the bundled `executable` with `args`. Content resolves from the bundle as it does for
/// players, except in builds that hot reload (`--features dev`): they read the desktop cook,
/// `target/content/desktop/`, which `cargo xtask content` rewrites while they run, since the
/// bundle's copy never changes. A `SAPPHIRE_CONTENT_DIR` the caller set, `inherited`, wins.
fn launch_command(
    executable: PathBuf,
    args: &[String],
    options: &Options,
    inherited: Option<&OsStr>,
) -> Command {
    let mut command = Command::new(executable);
    command.args(args);
    // The platform layer ignores an empty one too.
    if super::takes_recooks(options) && inherited.is_none_or(OsStr::is_empty) {
        command.env(CONTENT_DIR_ENV, content::cooked_dir(Cook::Desktop));
    }
    command
}

/// Extra environment for building for macOS. SDL's CMake build fails without an explicit
/// SDK on some Xcode and Command Line Tools combinations (seen with Xcode 26.2 on macOS
/// 27), so SDKROOT is set unless the user already has. The deployment target comes from
/// .cargo/config.toml.
pub fn build_env() -> anyhow::Result<Vec<(&'static str, OsString)>> {
    if std::env::var_os("SDKROOT").is_some() {
        return Ok(Vec::new());
    }
    let sdk = workspace::output(Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]))?;
    Ok(vec![("SDKROOT", sdk.trim().into())])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `SAPPHIRE_CONTENT_DIR` a run of a build with `features` sets, if any, given the
    /// caller's `inherited` one.
    fn content_dir(features: &[&str], inherited: Option<&str>) -> Option<PathBuf> {
        let options = Options {
            features: features.iter().map(|&feature| feature.to_owned()).collect(),
            ..Options::default()
        };
        let args = ["--frames".to_owned(), "10".to_owned()];
        let command =
            launch_command("App.app/Contents/MacOS/app".into(), &args, &options, inherited.map(OsStr::new));
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["--frames", "10"]);
        let mut envs = command.get_envs();
        let content = envs.next().map(|(name, value)| {
            assert_eq!(name, CONTENT_DIR_ENV);
            PathBuf::from(value.expect("set, not removed"))
        });
        assert!(envs.next().is_none(), "only the content directory is set");
        content
    }

    #[test]
    fn hot_reloading_runs_read_the_cook_not_the_bundle() {
        let cooked = Some(content::cooked_dir(Cook::Desktop));
        assert!(cooked.as_ref().is_some_and(|dir| dir.ends_with("target/content/desktop")));
        assert_eq!(content_dir(&["dev"], None), cooked);
        assert_eq!(content_dir(&["tracy", "dev"], None), cooked);
        assert_eq!(content_dir(&["dev"], Some("")), cooked, "an empty one is no choice");
        // The caller's choice stands, and builds without hot reload read the bundle as players do.
        assert_eq!(content_dir(&["dev"], Some("/elsewhere")), None);
        assert_eq!(content_dir(&[], None), None);
        assert_eq!(content_dir(&["tracy"], None), None);
    }
}
