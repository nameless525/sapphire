//! `cargo xtask package`: the game built for a store (M4 S1), the one route every store's package takes.
//!
//! A package is the game (`game_app`) built in the `shipping` profile, with the cook for its platform, the
//! version and build number ([`crate::version`]) everywhere a platform reads them, the identity
//! ([`super::IDENTITY_FILE`]) and the project's icon compiled for the platform. It's staged under
//! `<out>/partial/<platform>/` and moved to `<out>/<platform>/` whole, `<out>` being `target/package/`
//! unless `--out` says otherwise:
//!
//! | Platform | What's there |
//! |---|---|
//! | `ios` | `<Name>.xcarchive` ([`super::ios_store`]): the app and its dSYM, as Xcode's Organizer reads them, and `checks/` |
//! | `ios-sim` | `<Name>.app`: the executable, the mobile cook, Info.plist, `en.lproj` and `ja.lproj`, and actool's `Assets.car` with the icon's keys merged into Info.plist; with the store's keys and privacy manifest, signed ad hoc with the game's entitlements |
//! | `macos` | `<Name>.app`: as iOS's, nested under `Contents/`, with the desktop cook, and actool's `AppIcon.icns` too; signed ad hoc |
//! | `android` | `<Name>.aab` from Gradle's `bundleRelease`, offline, checked ([`super::aab`]), and `icons/play_store_512.png`; in `checks/`, the APKs Play would make of it for local testing, their badging and release lint's report |
//! | `windows`, `linux` | `<Name>/`: the executable and the desktop cook; Linux's icon beside them, and Windows' `.ico` in `icons/` |
//! | every one | `package.json`: the app, version, build number and kind, commit, identity and files; and `symbols/`, what the link split out: the dSYM (in the archive for `ios`), the unstripped Android library, the PDB or the `.dwp` |
//!
//! It makes what every store's package shares. What a store checks is its own package's: S2 wraps the
//! iOS app in an `.xcarchive` with its store keys and privacy manifest, S3 signs the AAB (with the upload
//! key only when the environment names it) and checks it, S4 makes Steam's depots, signs macOS with the
//! hardened runtime and links Windows' resources, and C1 strips the binaries and writes the symbol store.
//! Nothing here uploads or notarizes.
//!
//! Shipping builds link with fat LTO, so a package takes minutes and gigabytes: run it through the gate
//! slots (`slot --all --load 8`), never on a PR's CI (m4-plan §3, "CI").

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use anyhow::{Context as _, anyhow, ensure};
use serde_json::json;

use super::apple::{self, BundleLayout};
use super::{App, BundleClaim, GAME, aab, android, desktop, ios_store};
use crate::content::{self, Cook};
use crate::version::{Build, Kind};
use crate::workspace::{self, BuildProfile, CargoBuild};

/// A platform a store package is made for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Ios,
    /// The same app for the Simulator: what S2's recipe runs headless.
    IosSim,
    Android,
    MacOs,
    Windows,
    Linux,
}

impl Target {
    const ALL: [Self; 6] = [Self::Ios, Self::IosSim, Self::Android, Self::MacOs, Self::Windows, Self::Linux];

    pub fn name(self) -> &'static str {
        match self {
            Self::Ios => "ios",
            Self::IosSim => "ios-sim",
            Self::Android => "android",
            Self::MacOs => "macos",
            Self::Windows => "windows",
            Self::Linux => "linux",
        }
    }

    fn cook(self) -> Cook {
        match self {
            Self::Ios | Self::IosSim | Self::Android => Cook::Mobile,
            Self::MacOs | Self::Windows | Self::Linux => Cook::Desktop,
        }
    }

    /// The host operating systems (`std::env::consts::OS`) that can build it: Apple's tools are on macOS
    /// alone, and the desktops build natively, with no cross toolchain.
    fn hosts(self) -> &'static [&'static str] {
        match self {
            Self::Ios | Self::IosSim | Self::MacOs => &["macos"],
            Self::Android => &["macos", "linux"],
            Self::Windows => &["windows"],
            Self::Linux => &["linux"],
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Target {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        Self::ALL.into_iter().find(|target| target.name() == s).ok_or_else(|| {
            let names: Vec<_> = Self::ALL.iter().map(|target| target.name()).collect();
            anyhow!("unknown platform `{s}`; expected one of: {}", names.join(", "))
        })
    }
}

/// Where packages go unless `--out` says: `target/package/`.
pub fn default_out() -> PathBuf {
    workspace::output_dir().join("package")
}

/// Packages the game for `target` under `out`, and returns where. `dev` allows a build that isn't a
/// release's, marked dev ([`crate::version`]).
pub fn package(target: Target, out: &Path, dev: bool) -> anyhow::Result<PathBuf> {
    let host = std::env::consts::OS;
    ensure!(
        target.hosts().contains(&host),
        "a {target} package builds on {} only, not {host}",
        target.hosts().join(" or ")
    );
    let app = App::load(GAME)?;
    let build = Build::for_package(dev)?;
    eprintln!("packaging {} {} for {target}: {build}", app.package, app.version);
    if let Kind::Dev(why) = &build.kind {
        eprintln!("warning: a DEV package, for no store: {why}");
    }
    if app.placeholder {
        eprintln!(
            "warning: {} is a placeholder identity ({}): no store will take this package until the maintainer \
             chooses the game's",
            app.bundle_id,
            super::IDENTITY_FILE
        );
    }

    let claim = BundleClaim::take_in(out.to_owned())?;
    let stage = claim.staging(target.name())?;
    workspace::remove_dir_if_exists(&stage)?;
    fs::create_dir_all(&stage).with_context(|| format!("failed to create {}", stage.display()))?;
    let icons = stage.with_extension("icons");
    super::make_icons(&icons)?;
    let content = content::cook(target.cook())?;
    let env: Vec<(&str, OsString)> =
        build.env(&app.version).into_iter().map(|(name, value)| (name, value.into())).collect();

    let made = match target {
        Target::Ios | Target::IosSim | Target::MacOs => {
            apple_app(target, &app, &build, &env, &content.dir, &icons, &stage)?
        }
        Target::Android => {
            let work = stage.with_extension("gradle");
            let packaged = android::package(&app, build.number, &env, &content.dir, &work)?;
            let aab = format!("{}.aab", app.bundle_name);
            copy(&packaged.aab, &stage.join(&aab))?;
            let library = packaged.library.file_name().context("the library has a name")?;
            copy(&packaged.library, &stage.join("symbols").join(library))?;
            copy(&icons.join("android/play_store_512.png"), &stage.join("icons/play_store_512.png"))?;
            let checked = &packaged.checked;
            let (apks, badging, lint) =
                ("checks/local-testing.apks", "checks/badging.txt", "checks/lint-results-release.txt");
            copy(&checked.apks, &stage.join(apks))?;
            fs::write(stage.join(badging), &checked.badging).context("failed to write the badging")?;
            copy(&packaged.lint, &stage.join(lint))?;
            if checked.key == aab::Key::Debug {
                eprintln!(
                    "warning: {aab} is signed with the debug key, which Play refuses: set {} and its passwords \
                     to sign with the upload key",
                    aab::UPLOAD_KEYSTORE
                );
            }
            let checks = checked.summary(apks, badging, lint);
            workspace::remove_dir_if_exists(&work)?;
            Made { app: aab, symbols: vec![format!("symbols/{}", library.to_string_lossy())], checks }
        }
        Target::Windows | Target::Linux => desktop_app(target, &app, &env, &content.dir, &icons, &stage)?,
    };
    drop(content);
    workspace::remove_dir_if_exists(&icons)?;

    let manifest = manifest(target, &app, &build, &made);
    let path = stage.join("package.json");
    fs::write(&path, serde_json::to_string_pretty(&manifest).expect("JSON values serialize") + "\n")
        .with_context(|| format!("failed to write {}", path.display()))?;
    let packaged = claim.publish(&stage)?;
    Ok(packaged.path.clone())
}

/// What a package made, by path inside it: the app, what the link split out, and what its store's checks
/// found, where it has them.
#[derive(Debug, PartialEq, Eq)]
struct Made {
    app: String,
    symbols: Vec<String>,
    checks: serde_json::Value,
}

/// `package.json`: what a package is, for what reads it next (S2–S4, C1, and the patch manifest, which
/// takes the version and build number from it).
fn manifest(target: Target, app: &App, build: &Build, made: &Made) -> serde_json::Value {
    let (kind, why) = match &build.kind {
        Kind::Release => ("release", None),
        Kind::Dev(why) => ("dev", Some(why.as_str())),
    };
    json!({
        "app": app.package,
        "platform": target.name(),
        "version": app.version,
        "build": {
            "number": build.number,
            "id": build.id(&app.version),
            "kind": kind,
            "dev_because": why,
            "commit": build.commit,
        },
        "identity": {
            "bundle_id": app.bundle_id,
            "bundle_name": app.bundle_name,
            "display_name": { "en": app.display_name.en, "ja": app.display_name.ja },
            "placeholder": app.placeholder,
        },
        "profile": "shipping",
        "cook": target.cook().name(),
        "app_path": made.app,
        "symbols": made.symbols,
        "checks": made.checks,
    })
}

/// The executable cargo builds for `target` in the shipping profile, and the other files the link left
/// beside it (a dSYM, a PDB or a `.dwp`).
fn shipping_executable(
    app: &App,
    rust_target: Option<&str>,
    env: &[(&str, OsString)],
) -> anyhow::Result<(PathBuf, Vec<PathBuf>)> {
    let build = CargoBuild {
        package: &app.package,
        target: rust_target,
        profile: BuildProfile::Shipping,
        features: &[],
        env,
    };
    let mut command = build.command("build");
    command.args(["--bin", &app.package]);
    let mut split = Vec::new();
    let executable = workspace::build_artifact(&mut command, &app.package, |artifact| {
        let is_bin =
            artifact.target.name == app.package && artifact.target.kind.iter().any(|kind| kind == "bin");
        if !is_bin {
            return None;
        }
        let executable = artifact.executable?;
        split = artifact.filenames.into_iter().filter(|file| *file != executable).collect();
        Some(executable)
    })?;
    Ok((executable, split))
}

fn apple_app(
    target: Target,
    app: &App,
    build: &Build,
    env: &[(&str, OsString)],
    content: &Path,
    icons: &Path,
    stage: &Path,
) -> anyhow::Result<Made> {
    let (rust_target, template_dir, minimum_key, actool_platform) = match target {
        Target::Ios => (Some("aarch64-apple-ios"), "ios", "MinimumOSVersion", "iphoneos"),
        Target::IosSim => (Some("aarch64-apple-ios-sim"), "ios", "MinimumOSVersion", "iphonesimulator"),
        Target::MacOs => (None, "macos", "LSMinimumSystemVersion", "macosx"),
        Target::Android | Target::Windows | Target::Linux => unreachable!("an Apple target"),
    };
    // A macOS host build needs the SDK named (see `macos::build_env`); iOS's pick theirs by target.
    let mut env = env.to_vec();
    if target == Target::MacOs {
        env.extend(desktop::build_env()?);
    }
    let (executable, split) = shipping_executable(app, rust_target, &env)?;
    let (template_path, template) = super::info_plist_template(template_dir)?;
    apple::verify_minimum_os(&executable, &template, minimum_key)?;
    let info_plist = apple::render_info_plist(&template, app, build.number)
        .map_err(|e| e.context(format!("in {}", template_path.display())))?;

    let name = format!("{}.app", app.bundle_name);
    let root = stage.join(&name);
    let layout = if target == Target::MacOs { BundleLayout::macos(root) } else { BundleLayout::ios(root) };
    apple::assemble(&layout, app, &info_plist, &executable, content)?;
    let minimum = apple::plist_string(&template, minimum_key).context("the template names its minimum")?;
    let catalog = icons.join(if target == Target::MacOs { "macos" } else { "ios" }).join("Assets.xcassets");
    compile_icon(&catalog, &layout, actool_platform, minimum, &icons.join("actool"))?;
    match target {
        Target::MacOs => apple::codesign_ad_hoc(&layout.root)?,
        _ => ios_store::finish_app(&layout, app, target == Target::Ios, &stage.join("checks"))?,
    }

    let mut symbols = Vec::new();
    for file in split.iter().filter(|file| file.extension().is_some_and(|ext| ext == "dSYM")) {
        let file_name = file.file_name().context("a dSYM has a name")?;
        workspace::copy_dir(file, &stage.join("symbols").join(file_name))?;
        symbols.push(format!("symbols/{}", file_name.to_string_lossy()));
    }
    ensure!(!symbols.is_empty(), "the shipping build left no dSYM beside {}", executable.display());
    if target == Target::Ios {
        let (archive, dsym) = ios_store::archive(app, build, stage, &name, &symbols)?;
        return Ok(Made { app: archive, symbols: vec![dsym], checks: serde_json::Value::Null });
    }
    Ok(Made { app: name, symbols, checks: serde_json::Value::Null })
}

/// Compiles the icon's asset catalog into the bundle's resources with `actool` (no Xcode project), and
/// merges the keys it names (`CFBundleIcons`, or `CFBundleIconFile` and `CFBundleIconName` on macOS) into
/// the bundle's Info.plist. `scratch` takes actool's partial plist.
fn compile_icon(
    catalog: &Path,
    layout: &BundleLayout,
    platform: &str,
    minimum: &str,
    scratch: &Path,
) -> anyhow::Result<()> {
    fs::create_dir_all(scratch).with_context(|| format!("failed to create {}", scratch.display()))?;
    let partial = scratch.join("partial.plist");
    let mut actool = Command::new("xcrun");
    actool
        .arg("actool")
        .arg(catalog)
        .arg("--compile")
        .arg(&layout.resources_dir)
        .args(["--platform", platform, "--minimum-deployment-target", minimum, "--app-icon", "AppIcon"])
        .arg("--output-partial-info-plist")
        .arg(&partial);
    if platform != "macosx" {
        actool.args(["--target-device", "iphone", "--target-device", "ipad"]);
    }
    actool.args(["--output-format", "human-readable-text", "--notices", "--warnings", "--errors"]);
    workspace::run(&mut actool)?;
    let merge = format!("Merge \"{}\"", partial.display());
    workspace::run(Command::new("/usr/libexec/PlistBuddy").args(["-c", &merge]).arg(&layout.info_plist))
}

fn desktop_app(
    target: Target,
    app: &App,
    env: &[(&str, OsString)],
    content: &Path,
    icons: &Path,
    stage: &Path,
) -> anyhow::Result<Made> {
    let (executable, split) = shipping_executable(app, None, env)?;
    let dir = stage.join(&app.bundle_name);
    let file_name = executable.file_name().context("the executable has a name")?;
    copy(&executable, &dir.join(file_name))?;
    workspace::copy_dir(content, &dir.join("content"))?;
    match target {
        Target::Linux => {
            copy(&icons.join("linux/icon_512.png"), &dir.join(format!("{}.png", app.bundle_name)))?
        }
        // S4 links it into the executable, with the manifest and VERSIONINFO.
        Target::Windows => copy(&icons.join("windows/app.ico"), &stage.join("icons/app.ico"))?,
        _ => unreachable!("a desktop target"),
    }
    let mut symbols = Vec::new();
    for file in &split {
        let name = file.file_name().context("a split file has a name")?;
        let dest = stage.join("symbols").join(name);
        if file.is_dir() {
            workspace::copy_dir(file, &dest)?
        } else {
            copy(file, &dest)?
        }
        symbols.push(format!("symbols/{}", name.to_string_lossy()));
    }
    Ok(Made { app: app.bundle_name.clone(), symbols, checks: serde_json::Value::Null })
}

/// Copies a file, creating the directory it goes in.
fn copy(from: &Path, to: &Path) -> anyhow::Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::copy(from, to).with_context(|| format!("failed to copy {} to {}", from.display(), to.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platforms::test_game;

    #[test]
    fn parses_targets_and_their_hosts() {
        for target in Target::ALL {
            assert_eq!(target.name().parse::<Target>().unwrap(), target);
        }
        let error = "desktop".parse::<Target>().unwrap_err().to_string();
        assert_eq!(
            error,
            "unknown platform `desktop`; expected one of: ios, ios-sim, android, macos, windows, linux"
        );
        assert_eq!(Target::Ios.cook(), Cook::Mobile);
        assert_eq!(Target::Linux.cook(), Cook::Desktop);
        assert_eq!(Target::Windows.hosts(), ["windows"]);
        assert!(default_out().ends_with("target/package"));
    }

    /// `package.json` carries the one version and build number, and says what isn't a release's or a
    /// chosen identity's, for whatever reads it next.
    #[test]
    fn the_manifest_says_what_the_package_is() {
        let app = test_game();
        let made = Made {
            app: "SapphireGame.app".into(),
            symbols: vec!["symbols/game_app.dSYM".into()],
            checks: json!({ "signed_with": "debug" }),
        };
        let release = Build { number: 4123, kind: Kind::Release, commit: "1a2b3c4d".into() };
        let written = manifest(Target::Ios, &app, &release, &made);
        assert_eq!(written["version"], "0.1.0");
        assert_eq!(written["build"]["number"], 4123);
        assert_eq!(written["build"]["id"], "0.1.0+4123");
        assert_eq!(written["build"]["kind"], "release");
        assert!(written["build"]["dev_because"].is_null());
        assert_eq!(written["identity"]["bundle_id"], "com.example.sapphire.game");
        assert_eq!(written["identity"]["display_name"]["ja"], "サファイア");
        assert_eq!(written["identity"]["placeholder"], true);
        assert_eq!(written["cook"], "mobile");
        assert_eq!(written["symbols"][0], "symbols/game_app.dSYM");
        assert_eq!(written["checks"]["signed_with"], "debug");

        let dev = Build { kind: Kind::Dev("off main".into()), ..release };
        let written = manifest(Target::MacOs, &app, &dev, &made);
        assert_eq!(written["build"]["kind"], "dev");
        assert_eq!(written["build"]["dev_because"], "off main");
        assert_eq!(written["build"]["id"], "0.1.0+4123.dev.1a2b3c4");
    }
}
