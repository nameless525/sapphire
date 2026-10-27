//! iOS devices: bundle a flat `.app` for `aarch64-apple-ios`, sign it for development, then
//! install and launch it with `devicectl` (Xcode 15 and later), the console attached.
//!
//! Signing takes what only an Apple developer account gives:
//!
//! - `--identity <name or SHA-1>`: an Apple Development certificate in the login keychain
//!   (`security find-identity -v -p codesigning` lists them);
//! - `--profile <file>`: a development provisioning profile whose App ID covers the bundle
//!   identifier (exactly, or with a wildcard) and that lists the device. Xcode makes one for a
//!   free personal team.
//!
//! The profile is embedded as `embedded.mobileprovision` and its entitlements signed into the
//! app. Without either, `bundle` signs ad hoc: everything is built and checked, but no device
//! installs it. `run` needs both, and `--device <name or UDID>`, which
//! `xcrun devicectl list devices` lists. The M1 exit run's runbook (`docs/m1-exit-run.md`) has
//! the steps.
//!
//! As for the simulator, this bundles the cargo-built executable directly; the spec's Xcode
//! project (§7.3) comes with store builds.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, bail, ensure};

use super::apple::{self, BundleLayout};
use super::{App, Options, Platform};
use crate::content::{self, Cook};
use crate::workspace;

const TARGET: &str = "aarch64-apple-ios";

/// Builds `target/ios/<Name>.app` with the mobile cook in `content/`, signed with `--identity`
/// and `--profile`, or ad hoc without them.
pub fn bundle(app: &App, options: &Options) -> anyhow::Result<PathBuf> {
    super::require_macos_host(Platform::Ios)?;
    let sign_with = signing(options)?;
    let content = content::cook(Cook::Mobile)?;
    // As for the simulator: cargo and CMake pick the device SDK for the target.
    let executable = super::executable(app, Some(TARGET), &[], options)?;
    let (template_path, template) = super::info_plist_template("ios")?;
    apple::verify_minimum_os(&executable, &template, "MinimumOSVersion")?;
    let info_plist = apple::render_info_plist(&template, app)
        .map_err(|e| e.context(format!("in {}", template_path.display())))?;

    let layout = BundleLayout::ios(super::bundle_dir(Platform::Ios).join(format!("{}.app", app.bundle_name)));
    apple::assemble(&layout, app, &info_plist, &executable, &content.dir)?;
    match sign_with {
        Some((identity, profile)) => sign(&layout.root, app, identity, profile)?,
        None => {
            apple::codesign_ad_hoc(&layout.root)?;
            eprintln!("signed ad hoc, which no device installs: --identity and --profile sign it for one");
        }
    }
    eprintln!("bundled {}", super::display(&layout.root));
    Ok(layout.root)
}

/// Bundles and signs, installs on `--device` and launches with the app's console attached
/// until it exits.
pub fn run(app: &App, options: &Options, args: &[String]) -> anyhow::Result<()> {
    let device = options
        .device
        .as_deref()
        .context("--device <name or UDID> picks the device: `xcrun devicectl list devices` lists them")?;
    ensure!(
        signing(options)?.is_some(),
        "a device installs only apps signed for it: give --identity and --profile"
    );
    let bundle = bundle(app, options)?;
    let mut install = Command::new("xcrun");
    install.args(["devicectl", "device", "install", "app", "--device", device]).arg(&bundle);
    let mut launch = Command::new("xcrun");
    launch.args(["devicectl", "device", "process", "launch", "--device", device, "--console"]);
    // The app's arguments follow `--`, so devicectl takes none of them for its own options.
    launch.args(["--terminate-existing", &app.bundle_id, "--"]).args(args);
    if options.dry_run {
        for command in [&install, &launch] {
            eprintln!("dry run: {}", workspace::shell_line(command));
        }
        return Ok(());
    }
    workspace::run(&mut install)?;
    eprintln!("launching {} on {device}; Ctrl-C ends it", app.bundle_id);
    workspace::run(&mut launch)
}

/// The identity and profile to sign with, or `None` for ad hoc.
fn signing(options: &Options) -> anyhow::Result<Option<(&str, &Path)>> {
    match (&options.identity, &options.profile) {
        (Some(identity), Some(profile)) => {
            ensure!(profile.is_file(), "--profile {} is not a file", profile.display());
            Ok(Some((identity, profile)))
        }
        (None, None) => Ok(None),
        _ => bail!("--identity and --profile go together"),
    }
}

/// Embeds `profile` in `bundle` and signs it with `identity` and the profile's entitlements,
/// after checking that the profile's App ID covers the app's bundle identifier.
fn sign(bundle: &Path, app: &App, identity: &str, profile: &Path) -> anyhow::Result<()> {
    let scratch = super::bundle_dir(Platform::Ios).join("signing");
    fs::create_dir_all(&scratch).with_context(|| format!("failed to create {}", scratch.display()))?;
    // A profile is a signed plist (CMS): decoded, its entitlements are what the app is signed
    // with.
    let decoded = scratch.join("profile.plist");
    workspace::run(Command::new("security").args(["cms", "-D", "-i"]).arg(profile).arg("-o").arg(&decoded))?;
    let entitlements = scratch.join("entitlements.plist");
    workspace::run(
        Command::new("plutil")
            .args(["-extract", "Entitlements", "xml1", "-o"])
            .arg(&entitlements)
            .arg(&decoded),
    )?;
    let app_id = workspace::output(
        Command::new("plutil")
            .args(["-extract", "Entitlements.application-identifier", "raw", "-o", "-"])
            .arg(&decoded),
    )?;
    ensure!(
        app_id_covers(app_id.trim(), &app.bundle_id),
        "{} is for App ID {}, not {}: make a profile for this bundle identifier (or change bundle-id in \
         the app's [package.metadata.sapphire] if Apple gave it to another team)",
        profile.display(),
        app_id.trim(),
        app.bundle_id
    );
    fs::copy(profile, bundle.join("embedded.mobileprovision"))
        .with_context(|| format!("failed to embed {}", profile.display()))?;
    workspace::run(
        Command::new("codesign")
            .args(["--force", "--sign", identity, "--timestamp=none", "--entitlements"])
            .arg(&entitlements)
            .arg(bundle),
    )
}

/// Whether a profile's `application-identifier` (`TEAMID.dev.sapphire.sandbox`, or a wildcard
/// such as `TEAMID.*` or `TEAMID.dev.*`) covers `bundle_id`.
fn app_id_covers(application_id: &str, bundle_id: &str) -> bool {
    let Some((_team, pattern)) = application_id.split_once('.') else { return false };
    match pattern.strip_suffix('*') {
        Some(prefix) => bundle_id.starts_with(prefix),
        None => pattern == bundle_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_ids_cover_their_bundle_identifiers_and_wildcards() {
        assert!(app_id_covers("ABCDE12345.dev.sapphire.sandbox", "dev.sapphire.sandbox"));
        assert!(app_id_covers("ABCDE12345.*", "dev.sapphire.sandbox"));
        assert!(app_id_covers("ABCDE12345.dev.sapphire.*", "dev.sapphire.sandbox"));
        assert!(!app_id_covers("ABCDE12345.dev.sapphire.game", "dev.sapphire.sandbox"));
        assert!(!app_id_covers("ABCDE12345.com.*", "dev.sapphire.sandbox"));
        assert!(!app_id_covers("dev", "dev.sapphire.sandbox"), "no team");
    }

    #[test]
    fn identity_and_profile_go_together() {
        let options = |identity: Option<&str>, profile: Option<&str>| Options {
            identity: identity.map(str::to_owned),
            profile: profile.map(PathBuf::from),
            ..Options::default()
        };
        assert!(signing(&options(None, None)).unwrap().is_none(), "ad hoc");
        assert!(signing(&options(Some("Apple Development"), None)).is_err());
        assert!(signing(&options(None, Some("dev.mobileprovision"))).is_err());
        let missing = options(Some("Apple Development"), Some("/no/such.mobileprovision"));
        assert!(signing(&missing).unwrap_err().to_string().contains("not a file"));
    }
}
