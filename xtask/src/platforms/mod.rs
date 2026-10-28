//! `cargo xtask bundle` and `cargo xtask run`, one module per target platform.
//!
//! Each platform module exposes `bundle(app, options)` (assemble something installable)
//! and `run(app, options, args)` (build, cook, deploy and launch).
//!
//! Store packaging (`xtask package`, M4) is not here yet. It must decide how debug info
//! reaches Sentry and leaves the shipped binaries: split DWARF (`split-debuginfo`, `.dwp`)
//! or `objcopy --only-keep-debug` on Linux and Android, dSYMs on Apple, PDBs on Windows,
//! then stripping. Bundles here keep what the cargo profile produces; only AGP strips the
//! Android library.

mod android;
mod apple;
mod desktop;
mod ios;
mod ios_sim;
mod macos;

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use anyhow::{Context as _, anyhow, bail, ensure};
use serde::Deserialize;

use crate::content;
use crate::workspace::{self, BuildProfile, CargoBuild};

/// A deployment target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// The host: Windows, Linux or macOS, without platform packaging.
    Desktop,
    MacOs,
    IosSim,
    /// An iPhone or iPad, signed for development.
    Ios,
    Android,
}

impl Platform {
    const ALL: [Self; 5] = [Self::Desktop, Self::MacOs, Self::IosSim, Self::Ios, Self::Android];

    pub fn name(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::MacOs => "macos",
            Self::IosSim => "ios-sim",
            Self::Ios => "ios",
            Self::Android => "android",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Platform {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        Self::ALL.into_iter().find(|p| p.name() == s).ok_or_else(|| {
            let names: Vec<_> = Self::ALL.iter().map(|p| p.name()).collect();
            anyhow!("unknown platform `{s}`; expected one of: {}", names.join(", "))
        })
    }
}

/// The M0 app's package. The game app joins it later.
pub const SANDBOX: &str = "sapphire_sandbox";

/// A shipping app and its platform identity. Everything but the package name comes from
/// the package's Cargo.toml (`version` and `[package.metadata.sapphire]`), so every
/// platform reports the same identity and version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    /// Cargo package and binary name; also the executable name inside bundles, and the
    /// Android library name (`lib<package>.so`).
    pub package: String,
    /// `CFBundleShortVersionString` and Android `versionName`.
    pub version: String,
    /// Apple bundle identifier and Android application id.
    pub bundle_id: String,
    /// Bundle, APK and directory name, and `CFBundleName` (Apple allows 15 characters).
    pub bundle_name: String,
    /// The name under the icon.
    pub display_name: String,
    /// `CFBundleVersion` and Android `versionCode`: rises with every store upload.
    pub build: u32,
}

/// `[package.metadata.sapphire]` as written.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct Identity {
    bundle_id: String,
    bundle_name: String,
    display_name: String,
    build: u32,
}

impl App {
    /// Reads `package`'s identity from its manifest through `cargo metadata`.
    pub fn load(package: &str) -> anyhow::Result<Self> {
        #[derive(Deserialize)]
        struct Metadata {
            packages: Vec<Package>,
        }
        #[derive(Deserialize)]
        struct Package {
            name: String,
            version: String,
            metadata: Option<serde_json::Value>,
        }

        let json =
            workspace::output(workspace::cargo("metadata").args(["--format-version", "1", "--no-deps"]))?;
        let metadata: Metadata = serde_json::from_str(&json).context("unexpected `cargo metadata` output")?;
        let found = metadata
            .packages
            .into_iter()
            .find(|p| p.name == package)
            .with_context(|| format!("no workspace package `{package}`"))?;
        let identity = found
            .metadata
            .and_then(|metadata| metadata.get("sapphire").cloned())
            .with_context(|| format!("`{package}` has no [package.metadata.sapphire] table"))?;
        let identity = serde_json::from_value(identity)
            .with_context(|| format!("invalid [package.metadata.sapphire] in `{package}`"))?;
        Self::new(package, found.version, identity)
    }

    fn new(package: &str, version: String, identity: Identity) -> anyhow::Result<Self> {
        let Identity { bundle_id, bundle_name, display_name, build } = identity;
        // Valid as both an Apple bundle identifier and an Android application id.
        let segment_ok = |s: &str| {
            s.starts_with(|c: char| c.is_ascii_alphabetic())
                && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        };
        ensure!(
            bundle_id.split('.').count() >= 2 && bundle_id.split('.').all(segment_ok),
            "bundle-id `{bundle_id}`: use two or more dot-separated segments of letters, digits and `_`, \
             each starting with a letter"
        );
        ensure!(
            (1..=15).contains(&bundle_name.len()) && bundle_name.chars().all(|c| c.is_ascii_alphanumeric()),
            "bundle-name `{bundle_name}`: use 1 to 15 ASCII letters and digits"
        );
        ensure!(!display_name.trim().is_empty(), "display-name is empty");
        // XML (Info.plist, the Android manifest) can't hold most control characters at all.
        ensure!(
            !display_name.chars().any(char::is_control),
            "display-name {display_name:?} holds a control character"
        );
        ensure!(build >= 1, "build must be at least 1");
        // Apple accepts up to three dot-separated integers; no pre-release suffixes.
        ensure!(
            version.split('.').count() <= 3 && version.split('.').all(|p| p.parse::<u32>().is_ok()),
            "version `{version}` must be MAJOR.MINOR.PATCH numbers for Apple's CFBundleShortVersionString"
        );
        Ok(Self { package: package.to_owned(), version, bundle_id, bundle_name, display_name, build })
    }
}

/// Environment variable the platform layer reads to find content outside a bundle.
pub const CONTENT_DIR_ENV: &str = "SAPPHIRE_CONTENT_DIR";

/// Cooks for desktop and builds the app for the host, for commands that launch it themselves
/// (`golden`). Keep the [`content::Cooked`] alive while the app runs.
pub fn desktop_build(app: &App, options: &Options) -> anyhow::Result<(content::Cooked, PathBuf)> {
    desktop::prepare(app, options)
}

/// The environment host builds need: only macOS needs any (see `macos::build_env`).
pub fn host_build_env() -> anyhow::Result<Vec<(&'static str, OsString)>> {
    desktop::build_env()
}

/// Options shared by `bundle` and `run`.
#[derive(Debug, Default)]
pub struct Options {
    pub release: bool,
    /// Cargo features to build the app with (`--features dev` for the debug overlay).
    pub features: Vec<String>,
    /// Use this prebuilt executable instead of building the app.
    pub exe: Option<PathBuf>,
    /// `run --platform ios-sim`: also show the device in Simulator.app.
    pub show: bool,
    /// `--platform ios`: the signing identity (a name or SHA-1 `security find-identity` lists)
    /// and development provisioning profile; without them the bundle is signed ad hoc.
    pub identity: Option<String>,
    pub profile: Option<PathBuf>,
    /// `run --platform ios`: the device, by name or UDID (`xcrun devicectl list devices`).
    pub device: Option<String>,
    /// `run`: build, cook and bundle, then print the launch instead of performing it.
    pub dry_run: bool,
}

/// Assembles an installable bundle and returns its path.
pub fn bundle(platform: Platform, app: &App, options: &Options) -> anyhow::Result<PathBuf> {
    check_device_options(platform, options)?;
    match platform {
        Platform::Desktop => desktop::bundle(app, options),
        Platform::MacOs => macos::bundle(app, options),
        Platform::IosSim => ios_sim::bundle(app, options),
        Platform::Ios => ios::bundle(app, options),
        Platform::Android => android::bundle(app, options),
    }
}

/// Builds, cooks, deploys and launches, forwarding `args` to the app.
pub fn run(platform: Platform, app: &App, options: &Options, args: &[String]) -> anyhow::Result<()> {
    ensure!(!options.show || platform == Platform::IosSim, "--show applies only to --platform ios-sim");
    check_device_options(platform, options)?;
    match platform {
        Platform::Desktop => desktop::run(app, options, args),
        Platform::MacOs => macos::run(app, options, args),
        Platform::IosSim => ios_sim::run(app, options, args),
        Platform::Ios => ios::run(app, options, args),
        Platform::Android => android::run(app, options, args),
    }
}

/// Fails if `--identity`, `--profile` or `--device`, which only iOS devices take, are given for
/// another platform.
fn check_device_options(platform: Platform, options: &Options) -> anyhow::Result<()> {
    let given = options.identity.is_some() || options.profile.is_some() || options.device.is_some();
    ensure!(
        !given || platform == Platform::Ios,
        "--identity, --profile and --device apply only to --platform ios"
    );
    Ok(())
}

/// The app's executable: `--exe` if given, else a fresh cargo build for `target` with the
/// extra environment `env`.
fn executable(
    app: &App,
    target: Option<&str>,
    env: &[(&str, OsString)],
    options: &Options,
) -> anyhow::Result<PathBuf> {
    match &options.exe {
        Some(exe) => {
            ensure!(exe.is_file(), "--exe {} is not a file", exe.display());
            Ok(exe.clone())
        }
        None => workspace::build_binary(
            &CargoBuild {
                package: &app.package,
                target,
                profile: BuildProfile::app(options.release),
                features: &options.features,
                env,
            },
            &app.package,
        ),
    }
}

/// Whether the build hot reloads its content (the sandbox's `dev` feature, spec §5.10), so a
/// recook while it runs is the point rather than a hazard, and it should read the cook itself
/// rather than a copy.
fn takes_recooks(options: &Options) -> bool {
    options.features.iter().any(|feature| feature == "dev")
}

/// Runs `command` in the foreground, or with `--dry-run` prints it instead.
fn launch(command: &mut Command, options: &Options) -> anyhow::Result<()> {
    if options.dry_run {
        eprintln!("dry run: {}", workspace::shell_line(command));
        return Ok(());
    }
    workspace::run(command)
}

/// Where a platform's bundles go: `target/<platform>/`.
fn bundle_dir(platform: Platform) -> PathBuf {
    workspace::output_dir().join(platform.name())
}

fn require_macos_host(platform: Platform) -> anyhow::Result<()> {
    ensure!(std::env::consts::OS == "macos", "`--platform {platform}` needs a macOS host");
    Ok(())
}

/// The Info.plist template for a platform, from `platform/<dir>/Info.plist`.
fn info_plist_template(dir: &str) -> anyhow::Result<(PathBuf, String)> {
    let path = workspace::root().join("platform").join(dir).join("Info.plist");
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok((path, text)),
        Err(e) => bail!("failed to read {}: {e}", path.display()),
    }
}

fn display(path: &Path) -> String {
    path.strip_prefix(workspace::root()).unwrap_or(path).display().to_string()
}

/// The sandbox's real identity, read from its manifest without running cargo.
#[cfg(test)]
pub fn test_app() -> App {
    let manifest: toml::Table =
        toml::from_str(include_str!("../../../apps/sapphire_sandbox/Cargo.toml")).unwrap();
    let identity = manifest["package"]["metadata"]["sapphire"].clone();
    App::new(SANDBOX, "0.1.0".into(), identity.try_into().unwrap()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_platforms() {
        assert_eq!("desktop".parse::<Platform>().unwrap(), Platform::Desktop);
        assert_eq!("macos".parse::<Platform>().unwrap(), Platform::MacOs);
        assert_eq!("ios-sim".parse::<Platform>().unwrap(), Platform::IosSim);
        assert_eq!("ios".parse::<Platform>().unwrap(), Platform::Ios);
        assert_eq!("android".parse::<Platform>().unwrap(), Platform::Android);
        let err = "iphone".parse::<Platform>().unwrap_err().to_string();
        assert_eq!(err, "unknown platform `iphone`; expected one of: desktop, macos, ios-sim, ios, android");
    }

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

    #[test]
    fn reads_the_sandbox_identity() {
        let app = test_app();
        assert_eq!(app.package, "sapphire_sandbox");
        assert_eq!(app.bundle_id, "dev.sapphire.sandbox");
        assert_eq!(app.bundle_name, "SapphireSandbox");
        assert_eq!(app.display_name, "Sapphire Sandbox");
        assert!(app.build >= 1);
    }

    #[test]
    fn rejects_identities_some_platform_refuses() {
        let identity = |bundle_id: &str, bundle_name: &str| Identity {
            bundle_id: bundle_id.into(),
            bundle_name: bundle_name.into(),
            display_name: "Game".into(),
            build: 1,
        };
        assert!(App::new("game", "1.2.3".into(), identity("dev.studio.game", "Game")).is_ok());
        // Android forbids hyphens and leading digits; Apple caps CFBundleName at 15.
        assert!(App::new("game", "1.2.3".into(), identity("dev.studio.my-game", "Game")).is_err());
        assert!(App::new("game", "1.2.3".into(), identity("dev.1studio.game", "Game")).is_err());
        assert!(App::new("game", "1.2.3".into(), identity("game", "Game")).is_err());
        assert!(App::new("game", "1.2.3".into(), identity("dev.studio.game", "SixteenCharsLong")).is_err());
        assert!(App::new("game", "1.2.3-beta".into(), identity("dev.studio.game", "Game")).is_err());
        let zero = Identity { build: 0, ..identity("dev.studio.game", "Game") };
        assert!(App::new("game", "1.2.3".into(), zero).is_err());
        // Bundling escapes display names, so any text goes but control characters.
        let named = |display_name: &str| Identity {
            display_name: display_name.into(),
            ..identity("dev.studio.game", "Game")
        };
        assert!(App::new("game", "1.2.3".into(), named("Sword & Shield <$(NAME)>")).is_ok());
        assert!(App::new("game", "1.2.3".into(), named("Two\nLines")).is_err());
        assert!(App::new("game", "1.2.3".into(), named("Bell\u{7}")).is_err());
    }
}
