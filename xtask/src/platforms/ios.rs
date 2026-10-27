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
//! app: an exact App ID's as they are, a wildcard App ID's with the application identifier
//! and keychain group expanded to the app's own as Xcode does (no device has run a wildcard
//! profile's build yet). Without either, `bundle` signs ad hoc: everything is built and
//! checked, but no device installs it. `run` needs both, and `--device <name or UDID>`,
//! which `xcrun devicectl list devices` lists. The M1 exit run's runbook
//! (`docs/m1-exit-run.md`) has the steps.
//!
//! As for the simulator, this bundles the cargo-built executable directly; the spec's Xcode
//! project (§7.3) comes with store builds.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

/// Embeds `profile` in `bundle` and signs it with `identity` and the entitlements the profile
/// grants the app (see [`write_entitlements`]).
fn sign(bundle: &Path, app: &App, identity: &str, profile: &Path) -> anyhow::Result<()> {
    let scratch = super::bundle_dir(Platform::Ios).join("signing");
    fs::create_dir_all(&scratch).with_context(|| format!("failed to create {}", scratch.display()))?;
    // A profile is a signed plist (CMS): decoded, its entitlements are what the app is signed
    // with.
    let decoded = scratch.join("profile.plist");
    workspace::run(Command::new("security").args(["cms", "-D", "-i"]).arg(profile).arg("-o").arg(&decoded))?;
    let entitlements = scratch.join("entitlements.plist");
    write_entitlements(&decoded, &entitlements, &app.bundle_id)
        .with_context(|| format!("signing with {}", profile.display()))?;
    fs::copy(profile, bundle.join("embedded.mobileprovision"))
        .with_context(|| format!("failed to embed {}", profile.display()))?;
    workspace::run(
        Command::new("codesign")
            .args(["--force", "--sign", identity, "--timestamp=none", "--entitlements"])
            .arg(&entitlements)
            .arg(bundle),
    )
}

/// Writes to `out` the entitlements to sign the app with, from a decoded profile, after
/// checking that the profile's App ID covers `bundle_id`.
///
/// A profile for an exact App ID is signed with its entitlements as they are. A wildcard
/// profile's are what it allows, not what an app is signed with: it allows `PREFIX.*`, while a
/// signature names the app itself. So, as Xcode does, its `application-identifier` and a
/// keychain access group that is a wildcard covering the app become `PREFIX.<bundle id>` (see
/// [`concrete_app_id`] and [`concrete_keychain_groups`]); every other entitlement is signed as
/// the profile has it. Not yet checked on a device.
fn write_entitlements(decoded: &Path, out: &Path, bundle_id: &str) -> anyhow::Result<()> {
    workspace::run(
        Command::new("plutil").args(["-extract", "Entitlements", "xml1", "-o"]).arg(out).arg(decoded),
    )?;
    let app_id = workspace::output(
        Command::new("plutil").args(["-extract", "application-identifier", "raw", "-o", "-"]).arg(out),
    )?;
    let app_id = app_id.trim();
    let concrete = concrete_app_id(app_id, bundle_id).with_context(|| {
        format!(
            "the profile is for App ID {app_id}, not {bundle_id}: make a profile for this bundle identifier (or \
             change bundle-id in the app's [package.metadata.sapphire] if Apple gave it to another team)"
        )
    })?;
    // An exact App ID: nothing to expand, so the profile's entitlements are signed unchanged.
    if concrete == app_id {
        return Ok(());
    }
    workspace::run(
        Command::new("plutil").args(["-replace", "application-identifier", "-string", &concrete]).arg(out),
    )?;

    // Absent from some profiles: `plutil` then fails, saying so.
    let groups = Command::new("plutil")
        .args(["-extract", "keychain-access-groups", "json", "-o", "-"])
        .arg(out)
        .stderr(Stdio::null())
        .output()
        .context("failed to run plutil")?;
    if groups.status.success() {
        let groups: Vec<String> = serde_json::from_slice(&groups.stdout)
            .context("the profile's keychain-access-groups aren't strings")?;
        let concrete = concrete_keychain_groups(&groups, app_id, bundle_id);
        if concrete != groups {
            let json = serde_json::to_string(&concrete).expect("strings serialize");
            workspace::run(
                Command::new("plutil").args(["-replace", "keychain-access-groups", "-json", &json]).arg(out),
            )?;
        }
    }
    Ok(())
}

/// The application identifier an app with `bundle_id` is signed with under a profile for
/// `application_id`, or `None` if the profile doesn't cover it. Exact IDs
/// (`ABCDE12345.dev.sapphire.sandbox`) give themselves; wildcards (`ABCDE12345.*`,
/// `ABCDE12345.dev.*`) give their prefix and the bundle identifier. The prefix is the App
/// ID's, which is usually but not always the team ID.
fn concrete_app_id(application_id: &str, bundle_id: &str) -> Option<String> {
    let (prefix, pattern) = application_id.split_once('.').filter(|(prefix, _)| !prefix.is_empty())?;
    let covers = match pattern.strip_suffix('*') {
        Some(start) => bundle_id.starts_with(start),
        None => pattern == bundle_id,
    };
    covers.then(|| format!("{prefix}.{bundle_id}"))
}

/// The keychain access groups an app with `bundle_id` is signed with, from those of a profile
/// for the wildcard App ID `application_id`: a wildcard group with the App ID's prefix that
/// covers the app (`ABCDE12345.*`) becomes the app's own, `ABCDE12345.<bundle id>`, Xcode's
/// default. Other groups (`com.apple.token`, exact ones) stay, in order, without duplicates.
fn concrete_keychain_groups(groups: &[String], application_id: &str, bundle_id: &str) -> Vec<String> {
    let prefix = application_id.split_once('.').map_or(application_id, |(prefix, _)| prefix);
    let mut concrete: Vec<String> = Vec::with_capacity(groups.len());
    for group in groups {
        let expanded = match group.split_once('.') {
            Some((group_prefix, _)) if group_prefix == prefix && group.ends_with('*') => {
                concrete_app_id(group, bundle_id)
            }
            _ => None,
        };
        let group = expanded.unwrap_or_else(|| group.clone());
        if !concrete.contains(&group) {
            concrete.push(group);
        }
    }
    concrete
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUNDLE: &str = "dev.sapphire.sandbox";

    #[test]
    fn app_ids_cover_their_bundle_identifiers_and_wildcards() {
        let concrete = |application_id: &str| concrete_app_id(application_id, BUNDLE);
        let own = Some("ABCDE12345.dev.sapphire.sandbox".to_owned());
        assert_eq!(concrete("ABCDE12345.dev.sapphire.sandbox"), own, "exact: as is");
        assert_eq!(concrete("ABCDE12345.*"), own);
        assert_eq!(concrete("ABCDE12345.dev.sapphire.*"), own);
        assert_eq!(concrete("ABCDE12345.dev.*"), own);
        // The App ID's prefix, even where it isn't the team's.
        assert_eq!(concrete("PREFIX9876.*").as_deref(), Some("PREFIX9876.dev.sapphire.sandbox"));
        assert_eq!(concrete("ABCDE12345.dev.sapphire.game"), None);
        assert_eq!(concrete("ABCDE12345.com.*"), None);
        assert_eq!(concrete("ABCDE12345.dev.sapphire.sandbox.*"), None, "covers only longer identifiers");
        assert_eq!(concrete("dev"), None, "no prefix");
        assert_eq!(concrete(".*"), None, "an empty prefix");
    }

    #[test]
    fn keychain_wildcards_become_the_apps_own_group() {
        let groups = |groups: &[&str], application_id: &str| {
            let groups: Vec<String> = groups.iter().map(|&group| group.to_owned()).collect();
            concrete_keychain_groups(&groups, application_id, BUNDLE)
        };
        let own = "ABCDE12345.dev.sapphire.sandbox";
        // Under a wildcard App ID, plain or with a start: Xcode signs the app's own group.
        for app_id in ["ABCDE12345.*", "ABCDE12345.dev.*"] {
            assert_eq!(groups(&["ABCDE12345.*", "com.apple.token"], app_id), [own, "com.apple.token"]);
        }
        assert_eq!(groups(&["ABCDE12345.dev.*"], "ABCDE12345.*"), [own]);
        // Already the app's own, or both: once.
        assert_eq!(groups(&[own, "ABCDE12345.*"], "ABCDE12345.*"), [own]);
        // Another prefix's groups, groups that don't cover the app and exact ones stay as the profile has them.
        assert_eq!(
            groups(&["OTHERTEAM1.*", "ABCDE12345.com.*", "ABCDE12345.shared"], "ABCDE12345.*"),
            ["OTHERTEAM1.*", "ABCDE12345.com.*", "ABCDE12345.shared"]
        );
        assert!(groups(&[], "ABCDE12345.*").is_empty());
    }

    /// The decoded profile of a wildcard App ID, with the entitlements such profiles carry.
    const WILDCARD_PROFILE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Entitlements</key>
	<dict>
		<key>application-identifier</key>
		<string>ABCDE12345.*</string>
		<key>keychain-access-groups</key>
		<array>
			<string>ABCDE12345.*</string>
			<string>com.apple.token</string>
		</array>
		<key>get-task-allow</key>
		<true/>
		<key>com.apple.developer.team-identifier</key>
		<string>ABCDE12345</string>
	</dict>
	<key>Name</key>
	<string>Wildcard Development</string>
</dict>
</plist>
"#;

    /// `write_entitlements` through the real `plutil`, on macOS hosts.
    #[test]
    fn signs_wildcard_profiles_with_the_apps_own_identifier() {
        if std::env::consts::OS != "macos" {
            return;
        }
        let scratch = std::env::temp_dir().join(format!("xtask-entitlements-{}", std::process::id()));
        fs::create_dir_all(&scratch).unwrap();
        let extract = |file: &Path, key: &str, format: &str| {
            workspace::output(Command::new("plutil").args(["-extract", key, format, "-o", "-"]).arg(file))
                .unwrap()
        };
        // The profile with another application identifier.
        let for_app_id = |application_id: &str| {
            WILDCARD_PROFILE.replace(
                "<string>ABCDE12345.*</string>\n\t\t<key>",
                &format!("<string>{application_id}</string>\n\t\t<key>"),
            )
        };
        let cases = [("wildcard", WILDCARD_PROFILE.to_owned()), ("prefix", for_app_id("ABCDE12345.dev.*"))];
        for (name, profile) in cases {
            let decoded = scratch.join(format!("{name}.plist"));
            let out = scratch.join(format!("{name}.entitlements"));
            fs::write(&decoded, profile).unwrap();
            write_entitlements(&decoded, &out, BUNDLE).unwrap();
            assert_eq!(
                extract(&out, "application-identifier", "raw").trim(),
                "ABCDE12345.dev.sapphire.sandbox",
                "{name}"
            );
            assert_eq!(
                extract(&out, "keychain-access-groups", "json").trim(),
                r#"["ABCDE12345.dev.sapphire.sandbox","com.apple.token"]"#,
                "{name}"
            );
            // The rest as the profile has it. (`plutil` key paths escape a key's dots.)
            assert_eq!(extract(&out, "get-task-allow", "raw").trim(), "true", "{name}");
            assert_eq!(
                extract(&out, r"com\.apple\.developer\.team-identifier", "raw").trim(),
                "ABCDE12345",
                "{name}"
            );
        }

        // An exact App ID's profile is signed with its entitlements byte for byte as a plain
        // extract gives them, its wildcard keychain group included.
        let decoded = scratch.join("exact.plist");
        fs::write(&decoded, for_app_id("ABCDE12345.dev.sapphire.sandbox")).unwrap();
        let out = scratch.join("exact.entitlements");
        write_entitlements(&decoded, &out, BUNDLE).unwrap();
        let plain = scratch.join("exact.plain");
        workspace::run(
            Command::new("plutil").args(["-extract", "Entitlements", "xml1", "-o"]).arg(&plain).arg(&decoded),
        )
        .unwrap();
        assert_eq!(fs::read(&out).unwrap(), fs::read(&plain).unwrap());
        assert_eq!(
            extract(&out, "keychain-access-groups", "json").trim(),
            r#"["ABCDE12345.*","com.apple.token"]"#
        );

        // A profile for another App ID is refused, naming it.
        let decoded = scratch.join("other.plist");
        fs::write(&decoded, for_app_id("ABCDE12345.com.*")).unwrap();
        let error = write_entitlements(&decoded, &scratch.join("other.entitlements"), BUNDLE).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("the profile is for App ID ABCDE12345.com.*, not dev.sapphire.sandbox"),
            "{error}"
        );

        // Without keychain groups, none are added.
        let decoded = scratch.join("bare.plist");
        let bare = WILDCARD_PROFILE.replace(
            "\t\t<key>keychain-access-groups</key>\n\t\t<array>\n\t\t\t<string>ABCDE12345.*</string>\n\t\t\t<string>com.apple.token</string>\n\t\t</array>\n",
            "",
        );
        assert!(!bare.contains("keychain"));
        fs::write(&decoded, bare).unwrap();
        let out = scratch.join("bare.entitlements");
        write_entitlements(&decoded, &out, BUNDLE).unwrap();
        assert!(!fs::read_to_string(&out).unwrap().contains("keychain-access-groups"));
        workspace::remove_dir_if_exists(&scratch).unwrap();
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
