//! `cargo xtask bundle` and `cargo xtask run`, one module per target platform.
//!
//! Each platform module exposes `bundle(app, options)` (assemble something installable)
//! and `run(app, options, args)` (build, cook, deploy and launch).
//!
//! A platform has one place for its bundles, so a run claims it (`BundleClaim`) before it builds
//! anything, and keeps the claim, in its [`Bundle`], until what it built has been installed or has
//! finished running. Another run waits, saying so. A bundle is assembled and signed aside and only
//! then moved into place, so what's at that path is always whole.
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
use std::fs::{self, File};
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

/// The highest build number: the greatest `versionCode` Google Play takes, which also fits the `Int`
/// that `platform/android/app/build.gradle.kts` reads it as.
const MAX_BUILD: u32 = 2_100_000_000;

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
        // Valid as both an Apple bundle identifier (no `_`) and an Android application id (no `-`).
        let segment_ok = |s: &str| {
            s.starts_with(|c: char| c.is_ascii_alphabetic()) && s.chars().all(|c| c.is_ascii_alphanumeric())
        };
        ensure!(
            bundle_id.split('.').count() >= 2 && bundle_id.split('.').all(segment_ok),
            "bundle-id `{bundle_id}`: use two or more dot-separated segments of ASCII letters and digits, each \
             starting with a letter, which is all that Apple and Android both take"
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
        ensure!(
            (1..=MAX_BUILD).contains(&build),
            "build {build} must be from 1 to {MAX_BUILD}, the greatest versionCode Google Play takes"
        );
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
    /// Build the app in the `tools` profile: optimized, with no LTO, and incremental, so a one-line
    /// edit relinks in seconds where `release` takes minutes. Desktops only.
    pub incremental: bool,
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

/// An assembled bundle, and this run's claim on where it lies: its path holds this run's build, whole,
/// until this is dropped. Keep it until the bundle is installed or has finished running.
#[derive(Debug)]
pub struct Bundle {
    pub path: PathBuf,
    _claim: BundleClaim,
}

/// A run's claim on `target/<platform>/`, where the platform's bundles go along with what building
/// them shares (Android's generated sources and Gradle's outputs, say). Taken before building, since
/// two builds of one target with different features or profiles replace each other's executables as
/// well, and before cooking, never while holding content, so this and the content's locks can't wait
/// on each other.
#[derive(Debug)]
struct BundleClaim {
    dir: PathBuf,
    _lock: File,
}

impl BundleClaim {
    fn take(platform: Platform) -> anyhow::Result<Self> {
        Self::take_in(bundle_dir(platform))
    }

    /// Claims `dir`, waiting for any other run that has it. The operating system lets go of a run
    /// that ends, however it ends.
    fn take_in(dir: PathBuf) -> anyhow::Result<Self> {
        fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
        let path = dir.join("bundle.lock");
        let lock = workspace::lock_file(&path)?;
        workspace::wait_for_lock(
            &lock,
            File::try_lock,
            File::lock,
            &format!("to finish with {}", display(&dir)),
        )?;
        Ok(Self { dir, _lock: lock })
    }

    /// Where to assemble bundle `name`: beside where it goes, so moving it there is a rename, and under
    /// the same name, which signing tools read.
    fn staging(&self, name: &str) -> anyhow::Result<PathBuf> {
        let partial = self.dir.join("partial");
        fs::create_dir_all(&partial).with_context(|| format!("failed to create {}", partial.display()))?;
        Ok(partial.join(name))
    }

    /// Moves a finished bundle from [`Self::staging`] into place, over the last one.
    fn publish(self, staged: &Path) -> anyhow::Result<Bundle> {
        let name = staged.file_name().context("a staged bundle has a name")?;
        let path = self.dir.join(name);
        if staged.is_dir() {
            workspace::remove_dir_if_exists(&path)?;
        }
        fs::rename(staged, &path)
            .with_context(|| format!("failed to move {} to {}", staged.display(), path.display()))?;
        eprintln!("bundled {}", display(&path));
        Ok(Bundle { path, _claim: self })
    }
}

/// Assembles an installable bundle.
pub fn bundle(platform: Platform, app: &App, options: &Options) -> anyhow::Result<Bundle> {
    check_device_options(platform, options)?;
    check_build_options(platform, options)?;
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
    check_build_options(platform, options)?;
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

/// Fails if `--incremental` is given with `--release`, which is another build, or for a phone, whose
/// builds go through Gradle or signing and should be what a player runs.
fn check_build_options(platform: Platform, options: &Options) -> anyhow::Result<()> {
    if options.incremental {
        ensure!(!options.release, "--incremental and --release are different builds: pick one");
        ensure!(
            matches!(platform, Platform::Desktop | Platform::MacOs),
            "--incremental applies only to --platform desktop and macos"
        );
    }
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
                profile: if options.incremental {
                    BuildProfile::Tools
                } else {
                    BuildProfile::app(options.release)
                },
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
    match fs::read_to_string(&path) {
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
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::*;

    #[test]
    fn an_incremental_build_is_a_desktops_and_isnt_a_release() {
        let options = |release| Options { incremental: true, release, ..Options::default() };
        assert!(check_build_options(Platform::Desktop, &options(false)).is_ok());
        assert!(check_build_options(Platform::MacOs, &options(false)).is_ok());
        assert!(check_build_options(Platform::Android, &options(false)).is_err());
        assert!(check_build_options(Platform::Ios, &options(false)).is_err());
        assert!(check_build_options(Platform::IosSim, &options(false)).is_err());
        assert!(check_build_options(Platform::Desktop, &options(true)).is_err());
        assert!(check_build_options(Platform::Android, &Options::default()).is_ok());
    }

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

    /// A scratch directory of a test's own, removed afterwards.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("xtask-{name}-{}", std::process::id()));
            workspace::remove_dir_if_exists(&dir).unwrap();
            fs::create_dir_all(dir.join("content")).unwrap();
            fs::write(dir.join("content/shared.spack"), "pack").unwrap();
            fs::write(dir.join("debug"), "the debug build").unwrap();
            fs::write(dir.join("release"), "the release build").unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = workspace::remove_dir_if_exists(&self.0);
        }
    }

    /// What `apple::assemble` makes of `executable`, staged by `claim`.
    fn staged(claim: &BundleClaim, scratch: &Path, executable: &str) -> anyhow::Result<PathBuf> {
        let layout = apple::BundleLayout::ios(claim.staging("Game.app")?);
        let content = scratch.join("content");
        apple::assemble(&layout, &test_app(), "plist", &scratch.join(executable), &content)?;
        Ok(layout.root)
    }

    fn executable_of(bundle: &Path) -> String {
        fs::read_to_string(bundle.join(test_app().package)).unwrap()
    }

    /// One run as `run` goes about it: it claims the platform, bundles `executable`, says so, and when
    /// `install` says go (as after an emulator has taken its time booting) reads what an installer
    /// would. The claim goes with the bundle, after that.
    fn one_run(
        scratch: &Path,
        executable: &str,
        bundled: mpsc::Sender<()>,
        install: mpsc::Receiver<()>,
    ) -> String {
        let claim = BundleClaim::take_in(scratch.join("bundles")).unwrap();
        let stage = staged(&claim, scratch, executable).unwrap();
        let bundle = claim.publish(&stage).unwrap();
        bundled.send(()).unwrap();
        install.recv().unwrap();
        executable_of(&bundle.path)
    }

    /// A run paused between bundling and installing installs what it built, however long it takes, and
    /// the other run, of another profile, waits and then installs what it built.
    #[test]
    fn a_run_installs_what_it_bundled() {
        let scratch = Scratch::new("claim");
        let (first_bundled, first_is_bundled) = mpsc::channel();
        let (second_bundled, second_is_bundled) = mpsc::channel();
        let (go_first, first_go) = mpsc::channel();
        let (go_second, second_go) = mpsc::channel();
        let dir = scratch.0.as_path();
        // Everything moves in, so a failure here lets go of the runs' channels and they end with it.
        thread::scope(move |scope| {
            let first = scope.spawn(move || one_run(dir, "debug", first_bundled, first_go));
            first_is_bundled.recv().unwrap();
            let second = scope.spawn(move || one_run(dir, "release", second_bundled, second_go));
            assert_eq!(
                second_is_bundled.recv_timeout(Duration::from_millis(300)),
                Err(mpsc::RecvTimeoutError::Timeout),
                "the second run bundled over the first's"
            );
            go_first.send(()).unwrap();
            assert_eq!(first.join().unwrap(), "the debug build");
            second_is_bundled.recv().unwrap();
            go_second.send(()).unwrap();
            assert_eq!(second.join().unwrap(), "the release build");
        });
        assert_eq!(executable_of(&dir.join("bundles/Game.app")), "the release build");
    }

    /// An assembly or signing that fails leaves the last bundle where it was, whole, and lets go of the
    /// platform, and the next run clears what it left.
    #[test]
    fn a_failed_bundle_leaves_the_last_one_whole() {
        let scratch = Scratch::new("failed-bundle");
        let dir = scratch.0.join("bundles");
        let claim = BundleClaim::take_in(dir.clone()).unwrap();
        let stage = staged(&claim, &scratch.0, "debug").unwrap();
        let good = claim.publish(&stage).unwrap().path;
        assert!(!stage.exists(), "moved, not copied");

        // The executable can't be copied in, as after a build that vanished.
        let claim = BundleClaim::take_in(dir.clone()).unwrap();
        assert!(staged(&claim, &scratch.0, "missing").is_err());
        drop(claim);
        assert_eq!(executable_of(&good), "the debug build");
        assert_eq!(fs::read_to_string(good.join("content/shared.spack")).unwrap(), "pack");

        // Assembled whole, but it was never signed, so it never moved.
        let claim = BundleClaim::take_in(dir.clone()).unwrap();
        staged(&claim, &scratch.0, "release").unwrap();
        drop(claim);
        assert_eq!(executable_of(&good), "the debug build");

        let claim = BundleClaim::take_in(dir).unwrap();
        fs::write(claim.staging("Game.app").unwrap().join("left over"), "from the failed run").unwrap();
        let stage = staged(&claim, &scratch.0, "release").unwrap();
        assert!(!stage.join("left over").exists());
        let bundle = claim.publish(&stage).unwrap();
        assert_eq!(executable_of(&bundle.path), "the release build");
        assert!(!bundle.path.join("left over").exists(), "nothing of the last bundle stays");
    }

    /// An APK is a file, which the next one replaces in one move.
    #[test]
    fn a_bundle_may_be_a_file() {
        let scratch = Scratch::new("file-bundle");
        let dir = scratch.0.join("bundles");
        for contents in ["one", "two"] {
            let claim = BundleClaim::take_in(dir.clone()).unwrap();
            let apk = claim.staging("Game.apk").unwrap();
            fs::write(&apk, contents).unwrap();
            let bundle = claim.publish(&apk).unwrap();
            assert_eq!(bundle.path, dir.join("Game.apk"));
            assert_eq!(fs::read_to_string(&bundle.path).unwrap(), contents);
        }
    }

    /// Signing happens before the move, so a signature must not care where it was made.
    #[test]
    fn a_signature_survives_the_move_into_place() {
        if std::env::consts::OS != "macos" {
            return;
        }
        let scratch = Scratch::new("signed-bundle");
        let script = scratch.0.join("app");
        fs::write(&script, "#!/bin/sh\n").unwrap();
        let claim = BundleClaim::take_in(scratch.0.join("bundles")).unwrap();
        let layout = apple::BundleLayout::macos(claim.staging("Game.app").unwrap());
        apple::assemble(&layout, &test_app(), "", &script, &scratch.0.join("content")).unwrap();
        let plist = apple::render_info_plist(include_str!("../../../platform/macos/Info.plist"), &test_app());
        fs::write(&layout.info_plist, plist.unwrap()).unwrap();
        apple::codesign_ad_hoc(&layout.root).unwrap();
        let bundle = claim.publish(&layout.root).unwrap();
        workspace::run(Command::new("codesign").args(["--verify", "--deep", "--strict"]).arg(&bundle.path))
            .unwrap();
    }

    /// One bundle ID goes to Apple and Android alike, so it holds only what both take: Android has no
    /// hyphens and Apple no underscores, which leaves letters and digits.
    #[test]
    fn a_bundle_id_is_what_apple_and_android_both_take() {
        let identity = |bundle_id: &str| Identity {
            bundle_id: bundle_id.into(),
            bundle_name: "Game".into(),
            display_name: "Game".into(),
            build: 1,
        };
        for good in ["dev.sapphire.sandbox", "com.Studio2.Game3", "a.b"] {
            assert!(App::new("game", "1.2.3".into(), identity(good)).is_ok(), "{good}");
        }
        for bad in [
            "dev.sapphire_bad.sandbox",
            "dev.sapphire.sand_box",
            "dev._studio.game",
            "dev.my-studio.game",
            "dev.1studio.game",
            "dev..game",
            ".dev.game",
            "dev.game.",
            "dev.st udio.game",
            "dev.stüdio.game",
            "game",
            "",
        ] {
            let error = App::new("game", "1.2.3".into(), identity(bad)).unwrap_err().to_string();
            assert!(error.contains("bundle-id") && error.contains("letters and digits"), "{bad}: {error}");
        }
    }

    /// A build number becomes a `versionCode`, which Gradle reads as an `Int` and Google Play holds to
    /// less than that.
    #[test]
    fn a_build_number_is_what_android_takes() {
        let identity = |build: u32| Identity {
            bundle_id: "dev.studio.game".into(),
            bundle_name: "Game".into(),
            display_name: "Game".into(),
            build,
        };
        for good in [1, 2, MAX_BUILD] {
            assert_eq!(App::new("game", "1.2.3".into(), identity(good)).unwrap().build, good);
        }
        for bad in [0, MAX_BUILD + 1, i32::MAX as u32, i32::MAX as u32 + 1, u32::MAX] {
            let error = App::new("game", "1.2.3".into(), identity(bad)).unwrap_err().to_string();
            assert!(error.contains("build"), "{bad}: {error}");
        }
        assert!(i32::try_from(MAX_BUILD).is_ok(), "Gradle's `toInt` reads it");
    }
}
