//! `cargo xtask bundle` and `cargo xtask run`, one module per target platform.
//!
//! Each platform module exposes `bundle(app, options)` (assemble something installable)
//! and `run(app, options, args)` (build, cook, deploy and launch).
//!
//! A platform has one place for its bundles, so a run claims it (`BundleClaim`) before it builds
//! anything, and keeps the claim, in its [`Bundle`], until what it built has been installed or has
//! finished running. Another run waits, saying so, whichever app it builds. A bundle is assembled and
//! signed aside and only then moved into place, so what's at that path is always whole.
//!
//! The host's platforms and `golden` share one executable path across their builds, so each run launches
//! or bundles a copy of its own ([`HostExecutable`]).
//!
//! Store packages are `package`'s: the game in the `shipping` profile, with its build number, identity and
//! icon, staged for what each store's package adds. Development bundles here keep what the cargo profile
//! produces; only AGP strips the Android library.

mod aab;
mod android;
mod apple;
mod desktop;
mod host;
mod ios;
mod ios_sim;
mod macos;
mod package;

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

pub use host::HostExecutable;
pub use package::{Target as PackageTarget, default_out as default_package_dir, package};

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

/// The app `run` and `bundle` build unless `--app` names another, and the only one `golden` runs.
pub const SANDBOX: &str = "sapphire_sandbox";

/// The game: the app `package` packages for the stores.
pub const GAME: &str = "game_app";

/// Where every app's identity is written: bundle IDs, names and the game's placeholders.
pub const IDENTITY_FILE: &str = "platform/identity.toml";

/// An app and its platform identity. The version is its Cargo package's (the workspace's), and the rest
/// comes from [`IDENTITY_FILE`], so every platform reports the same identity and version. The build number
/// is the build's ([`crate::version`]), not the app's.
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
    /// The name under the icon, in each language.
    pub display_name: DisplayName,
    /// Whether the identity is a placeholder nobody has chosen yet, under a reserved namespace
    /// ([`PLACEHOLDER_NAMESPACES`]).
    pub placeholder: bool,
}

/// The name under the icon, in the languages the game ships: English, the development region, and
/// Japanese.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayName {
    pub en: String,
    pub ja: String,
}

impl DisplayName {
    /// Each language's code, as Apple's `.lproj` folders and Android's `values-<code>` name it, and its name.
    pub fn by_language(&self) -> [(&'static str, &str); 2] {
        [("en", &self.en), ("ja", &self.ja)]
    }
}

/// The reverse-DNS namespaces RFC 2606 reserves (example.com, .net and .org): a placeholder bundle ID lies
/// under one, and a chosen one never does, so neither passes for the other.
pub const PLACEHOLDER_NAMESPACES: [&str; 3] = ["com.example", "net.example", "org.example"];

/// An app's table in [`IDENTITY_FILE`], as written.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct Identity {
    #[serde(default)]
    placeholder: bool,
    bundle_id: String,
    bundle_name: String,
    display_name: DisplayName,
}

impl App {
    /// Reads `package`'s identity from [`IDENTITY_FILE`], and its version through `cargo metadata`.
    pub fn load(package: &str) -> anyhow::Result<Self> {
        let path = workspace::root().join(IDENTITY_FILE);
        let identities =
            fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
        let json =
            workspace::output(workspace::cargo("metadata").args(["--format-version", "1", "--no-deps"]))?;
        Self::from_sources(&identities, &json, package)
    }

    /// The app `package`: a table in `identities` ([`IDENTITY_FILE`]'s text) and a workspace package in
    /// `cargo metadata`'s output. Any other name fails, naming the apps there are.
    fn from_sources(identities: &str, metadata: &str, package: &str) -> anyhow::Result<Self> {
        #[derive(Deserialize)]
        struct Metadata {
            packages: Vec<Package>,
        }
        #[derive(Deserialize)]
        struct Package {
            name: String,
            version: String,
        }

        let mut identities: toml::Table =
            toml::from_str(identities).with_context(|| format!("invalid {IDENTITY_FILE}"))?;
        let Some(identity) = identities.remove(package) else {
            let known: Vec<&str> = identities.keys().map(String::as_str).collect();
            bail!("no app `{package}`; the apps {IDENTITY_FILE} names are: {}", known.join(", "));
        };
        let identity: Identity =
            identity.try_into().with_context(|| format!("invalid [{package}] in {IDENTITY_FILE}"))?;
        let metadata: Metadata =
            serde_json::from_str(metadata).context("unexpected `cargo metadata` output")?;
        let found = metadata.packages.into_iter().find(|p| p.name == package).with_context(|| {
            format!("{IDENTITY_FILE} names `{package}`, which isn't a package in this workspace")
        })?;
        Self::new(package, found.version, identity)
            .with_context(|| format!("invalid [{package}] in {IDENTITY_FILE}"))
    }

    fn new(package: &str, version: String, identity: Identity) -> anyhow::Result<Self> {
        let Identity { placeholder, bundle_id, bundle_name, display_name } = identity;
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
            placeholder == is_placeholder_id(&bundle_id),
            "{}",
            if placeholder {
                format!(
                    "bundle-id `{bundle_id}` is marked a placeholder, but a placeholder lies under {}, so nobody \
                     can take it for one somebody chose",
                    PLACEHOLDER_NAMESPACES.join(", ")
                )
            } else {
                format!(
                    "bundle-id `{bundle_id}` is a placeholder's, but the identity isn't marked \
                     `placeholder = true`: choose the app's own, or mark it"
                )
            }
        );
        ensure!(
            (1..=15).contains(&bundle_name.len()) && bundle_name.chars().all(|c| c.is_ascii_alphanumeric()),
            "bundle-name `{bundle_name}`: use 1 to 15 ASCII letters and digits"
        );
        for (language, name) in display_name.by_language() {
            ensure!(!name.trim().is_empty(), "display-name.{language} is empty");
            // XML (Info.plist, Android's resources) can't hold most control characters at all.
            ensure!(
                !name.chars().any(char::is_control),
                "display-name.{language} {name:?} holds a control character"
            );
        }
        // Apple accepts up to three dot-separated integers; no pre-release suffixes.
        ensure!(
            version.split('.').count() <= 3 && version.split('.').all(|p| p.parse::<u32>().is_ok()),
            "version `{version}` must be MAJOR.MINOR.PATCH numbers for Apple's CFBundleShortVersionString"
        );
        Ok(Self { package: package.to_owned(), version, bundle_id, bundle_name, display_name, placeholder })
    }
}

/// Whether `bundle_id` lies under a [`PLACEHOLDER_NAMESPACES`] namespace.
fn is_placeholder_id(bundle_id: &str) -> bool {
    PLACEHOLDER_NAMESPACES.iter().any(|namespace| {
        bundle_id.strip_prefix(namespace).is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
    })
}

/// Environment variable the platform layer reads to find content outside a bundle.
pub const CONTENT_DIR_ENV: &str = "SAPPHIRE_CONTENT_DIR";

/// Cooks for desktop and builds the app for the host, for commands that launch it themselves
/// (`golden`). Keep both alive while the app runs.
pub fn desktop_build(app: &App, options: &Options) -> anyhow::Result<(content::Cooked, HostExecutable)> {
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
/// on each other. Host builds share an executable with another platform too: [`HostExecutable`].
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

/// Fills a template's Xcode-style `$(NAME)` placeholders from `variables`, in one pass over the
/// template: a value is never scanned for placeholders. `push` appends a value as the file needs it
/// written. `file` names the template in errors.
fn fill_template(
    template: &str,
    file: &str,
    variables: &[(&str, &str)],
    push: fn(&mut String, &str),
) -> anyhow::Result<String> {
    let mut filled = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("$(") {
        filled.push_str(&rest[..start]);
        let token = &rest[start..];
        let Some(end) = token.find(')') else { bail!("unterminated placeholder in {file} template") };
        let name = &token["$(".len()..end];
        let Some((_, value)) = variables.iter().find(|(variable, _)| *variable == name) else {
            bail!("unknown placeholder $({name}) in {file} template");
        };
        push(&mut filled, value);
        rest = &token[end + 1..];
    }
    filled.push_str(rest);
    Ok(filled)
}

/// The project's icon: a square, opaque PNG of at least 1024 px, drawn by `sapphire_pipeline gen-icon`
/// until the maintainer replaces it with the game's own.
pub const ICON_FILE: &str = "platform/icon/icon.png";

/// Makes every platform's icon set from [`ICON_FILE`] under `out`, replacing what was there
/// (`sapphire_pipeline icons`).
fn make_icons(out: &Path) -> anyhow::Result<()> {
    let pipeline = content::pipeline()?;
    workspace::remove_dir_if_exists(out)?;
    workspace::run(
        Command::new(pipeline)
            .arg("icons")
            .arg("--source")
            .arg(workspace::root().join(ICON_FILE))
            .arg("--out")
            .arg(out),
    )
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

/// The app `package` as [`IDENTITY_FILE`] has it, at version 0.1.0, read without running cargo.
#[cfg(test)]
fn app_from_identity_file(package: &str) -> App {
    let mut identities: toml::Table =
        toml::from_str(include_str!("../../../platform/identity.toml")).unwrap();
    App::new(package, "0.1.0".into(), identities.remove(package).unwrap().try_into().unwrap()).unwrap()
}

/// The sandbox's real identity.
#[cfg(test)]
pub fn test_app() -> App {
    app_from_identity_file(SANDBOX)
}

/// The game's real identity: the one `package` packages.
#[cfg(test)]
pub fn test_game() -> App {
    app_from_identity_file(GAME)
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
        assert_eq!(app.display_name.en, "Sapphire Sandbox");
        assert!(!app.placeholder, "a development app's identity is its own");
    }

    /// The game's identity is a placeholder until the maintainer chooses one (m4-plan §7), and it says so.
    #[test]
    fn reads_the_games_placeholder_identity() {
        let app = test_game();
        assert_eq!(app.package, "game_app");
        assert_eq!(app.bundle_id, "com.example.sapphire.game");
        assert_eq!(app.bundle_name, "SapphireGame");
        assert_eq!(app.display_name, DisplayName { en: "Sapphire".into(), ja: "サファイア".into() });
        assert!(app.placeholder);
    }

    const IDENTITIES: &str = r#"
        [sapphire_sandbox]
        bundle-id = "dev.sapphire.sandbox"
        bundle-name = "SapphireSandbox"
        display-name = { en = "Sapphire Sandbox", ja = "サンドボックス" }

        [game_app]
        placeholder = true
        bundle-id = "com.example.sapphire.game"
        bundle-name = "SapphireGame"
        display-name = { en = "Sapphire", ja = "サファイア" }

        [lost_app]
        bundle-id = "dev.sapphire.lost"
        bundle-name = "Lost"
        display-name = { en = "Lost", ja = "Lost" }
    "#;

    /// `cargo metadata` as `App::load` reads it: the apps, and a library.
    const METADATA: &str = r#"{"packages": [
        {"name": "sapphire_sandbox", "version": "0.1.0"},
        {"name": "sapphire_render", "version": "0.1.0"},
        {"name": "game_app", "version": "0.2.0"}
    ]}"#;

    #[test]
    fn finds_each_app_in_the_identity_file_and_the_workspace() {
        let game = App::from_sources(IDENTITIES, METADATA, "game_app").unwrap();
        assert_eq!((game.bundle_id.as_str(), game.version.as_str()), ("com.example.sapphire.game", "0.2.0"));
        assert_eq!(game.display_name.ja, "サファイア");
        let sandbox = App::from_sources(IDENTITIES, METADATA, SANDBOX).unwrap();
        assert_eq!(sandbox.bundle_id, "dev.sapphire.sandbox");
    }

    /// A typo and a package with no identity fail alike, saying what would have worked; an identity for no
    /// package says so.
    #[test]
    fn an_unknown_app_names_the_known_ones() {
        for name in ["game", "sapphire_render"] {
            assert_eq!(
                App::from_sources(IDENTITIES, METADATA, name).unwrap_err().to_string(),
                format!(
                    "no app `{name}`; the apps platform/identity.toml names are: game_app, lost_app, sapphire_sandbox"
                )
            );
        }
        let error = App::from_sources(IDENTITIES, METADATA, "lost_app").unwrap_err().to_string();
        assert_eq!(error, "platform/identity.toml names `lost_app`, which isn't a package in this workspace");
    }

    #[test]
    fn a_broken_identity_is_reported_as_one() {
        let identities = "[game_app]\nbundle-id = \"com.example.game\"\n";
        let error = App::from_sources(identities, METADATA, "game_app").unwrap_err().to_string();
        assert_eq!(error, "invalid [game_app] in platform/identity.toml");
        // Fields nobody reads are mistakes, as a hand-raised build number would be now.
        let identities = IDENTITIES.replace("placeholder = true", "placeholder = true\nbuild = 7");
        assert!(App::from_sources(&identities, METADATA, "game_app").is_err());
    }

    fn identity(bundle_id: &str, bundle_name: &str) -> Identity {
        Identity {
            placeholder: false,
            bundle_id: bundle_id.into(),
            bundle_name: bundle_name.into(),
            display_name: DisplayName { en: "Game".into(), ja: "ゲーム".into() },
        }
    }

    #[test]
    fn rejects_identities_some_platform_refuses() {
        assert!(App::new("game", "1.2.3".into(), identity("dev.studio.game", "Game")).is_ok());
        // Android forbids hyphens and leading digits; Apple caps CFBundleName at 15.
        assert!(App::new("game", "1.2.3".into(), identity("dev.studio.my-game", "Game")).is_err());
        assert!(App::new("game", "1.2.3".into(), identity("dev.1studio.game", "Game")).is_err());
        assert!(App::new("game", "1.2.3".into(), identity("game", "Game")).is_err());
        assert!(App::new("game", "1.2.3".into(), identity("dev.studio.game", "SixteenCharsLong")).is_err());
        assert!(App::new("game", "1.2.3-beta".into(), identity("dev.studio.game", "Game")).is_err());
        // Bundling escapes display names, so any text goes but control characters, in either language.
        let named = |en: &str, ja: &str| Identity {
            display_name: DisplayName { en: en.into(), ja: ja.into() },
            ..identity("dev.studio.game", "Game")
        };
        assert!(App::new("game", "1.2.3".into(), named("Sword & Shield <$(NAME)>", "剣と盾")).is_ok());
        assert!(App::new("game", "1.2.3".into(), named("Two\nLines", "ゲーム")).is_err());
        assert!(App::new("game", "1.2.3".into(), named("Game", "ベル\u{7}")).is_err());
        assert!(App::new("game", "1.2.3".into(), named("Game", " ")).is_err());
    }

    /// A placeholder can't look like an identity somebody chose, and a chosen one can't still be a
    /// placeholder: what reaches a store is either marked or the maintainer's.
    #[test]
    fn a_placeholder_never_passes_for_a_chosen_identity() {
        let placeholder = |bundle_id: &str| Identity { placeholder: true, ..identity(bundle_id, "Game") };
        for reserved in ["com.example.sapphire.game", "com.example", "org.example.game", "net.example.a"] {
            assert!(
                App::new("game", "1.2.3".into(), placeholder(reserved)).unwrap().placeholder,
                "{reserved}"
            );
            let error = App::new("game", "1.2.3".into(), identity(reserved, "Game")).unwrap_err().to_string();
            assert!(error.contains("isn't marked `placeholder = true`"), "{reserved}: {error}");
        }
        // Real-looking, so never a placeholder: the namespace this project used before, a studio's, and
        // names that only start like the reserved ones.
        for real in ["dev.sapphire.game", "com.studio.game", "com.examples.game", "com.exampleco.game"] {
            assert!(!App::new("game", "1.2.3".into(), identity(real, "Game")).unwrap().placeholder, "{real}");
            let error = App::new("game", "1.2.3".into(), placeholder(real)).unwrap_err().to_string();
            assert!(
                error.contains("is marked a placeholder, but a placeholder lies under"),
                "{real}: {error}"
            );
        }
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
        let plist =
            apple::render_info_plist(include_str!("../../../platform/macos/Info.plist"), &test_app(), 1);
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
        for good in ["dev.sapphire.sandbox", "com.Studio2.Game3", "a.b"] {
            assert!(App::new("game", "1.2.3".into(), identity(good, "Game")).is_ok(), "{good}");
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
            let error = App::new("game", "1.2.3".into(), identity(bad, "Game")).unwrap_err().to_string();
            assert!(error.contains("bundle-id") && error.contains("letters and digits"), "{bad}: {error}");
        }
    }
}
