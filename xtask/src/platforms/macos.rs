//! macOS: run the host build directly, or bundle it as a `.app`.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

use super::apple::{self, BundleLayout};
use super::{App, Options, Platform};
use crate::content::{self, Cook};
use crate::workspace;

/// Environment variable the sandbox reads to find cooked content outside a bundle.
const CONTENT_DIR_ENV: &str = "SAPPHIRE_CONTENT_DIR";
/// Matches `LSMinimumSystemVersion` in `platform/macos/Info.plist` (spec §2).
const MINIMUM_MACOS: &str = "14.0";

/// Builds `target/macos/<Name>.app` with the desktop cook in `Contents/Resources/content`.
pub fn bundle(app: &App, options: &Options) -> anyhow::Result<PathBuf> {
    super::require_macos_host(Platform::MacOs)?;
    let content = content::cook(Cook::Desktop)?;
    let executable = super::executable(app, None, &build_env()?, options)?;
    let (template_path, template) = super::info_plist_template("macos")?;
    let info_plist = apple::render_info_plist(&template, app)
        .map_err(|e| e.context(format!("in {}", template_path.display())))?;

    let layout =
        BundleLayout::macos(super::bundle_dir(Platform::MacOs).join(format!("{}.app", app.bundle_name)));
    apple::assemble(&layout, app, &info_plist, &executable, &content)?;
    apple::codesign_ad_hoc(&layout.root)?;
    eprintln!("bundled {}", super::display(&layout.root));
    Ok(layout.root)
}

/// Cooks for desktop, builds the app for the host and runs it against the cooked
/// directory, without bundling.
pub fn run(app: &App, options: &Options, args: &[String]) -> anyhow::Result<()> {
    super::require_macos_host(Platform::MacOs)?;
    let content = content::cook(Cook::Desktop)?;
    let executable = super::executable(app, None, &build_env()?, options)?;
    workspace::run(Command::new(executable).args(args).env(CONTENT_DIR_ENV, content))
}

/// Environment for building the app. The deployment target makes C dependencies such as
/// SDL build for the minimum OS rather than the SDK's version. SDKROOT works around SDL's
/// CMake build failing without it (seen with Xcode 26.2 on macOS 27); one the user set
/// is kept.
fn build_env() -> anyhow::Result<Vec<(&'static str, OsString)>> {
    let mut env = vec![("MACOSX_DEPLOYMENT_TARGET", MINIMUM_MACOS.into())];
    if std::env::var_os("SDKROOT").is_none() {
        let sdk = workspace::output(Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]))?;
        env.push(("SDKROOT", sdk.trim().into()));
    }
    Ok(env)
}
