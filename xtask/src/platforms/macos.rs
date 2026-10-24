//! macOS: bundle the host build as a signed `.app`, and run the bundled executable.
//! `--platform desktop` runs the unbundled build instead.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

use super::apple::{self, BundleLayout};
use super::{App, Options, Platform};
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

/// Bundles, then runs the bundled executable in the foreground, so content resolves from
/// the bundle as it does for players and the console stays attached.
pub fn run(app: &App, options: &Options, args: &[String]) -> anyhow::Result<()> {
    let bundle = bundle(app, options)?;
    let executable = BundleLayout::macos(bundle).executable_dir.join(&app.package);
    super::launch(Command::new(executable).args(args), options)
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
