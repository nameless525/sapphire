//! `cargo xtask bundle` and `cargo xtask run`, one module per target platform.
//!
//! Each platform module exposes `bundle(app, options)` (assemble something installable)
//! and `run(app, options, args)` (build, cook, deploy and launch).

mod android;
mod apple;
mod ios_sim;
mod macos;

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{anyhow, ensure};

use crate::workspace::{self, CargoBuild};

/// A deployment target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    IosSim,
    Android,
}

impl Platform {
    const ALL: [Self; 3] = [Self::MacOs, Self::IosSim, Self::Android];

    pub fn name(self) -> &'static str {
        match self {
            Self::MacOs => "macos",
            Self::IosSim => "ios-sim",
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

/// A shipping app and its platform identity.
pub struct App {
    /// Cargo package and binary name; also the executable name inside bundles, and the
    /// Android library name (`lib<package>.so`).
    pub package: &'static str,
    /// Bundle directory and `CFBundleName` (Apple asks for at most 15 characters).
    pub bundle_name: &'static str,
    /// The name under the icon.
    pub display_name: &'static str,
    /// Apple bundle identifier and Android application id.
    pub bundle_id: &'static str,
}

/// The M0 app. The game app joins it later.
pub const SANDBOX: App = App {
    package: "sapphire_sandbox",
    bundle_name: "SapphireSandbox",
    display_name: "Sapphire Sandbox",
    bundle_id: "dev.sapphire.sandbox",
};

/// Options shared by `bundle` and `run`.
#[derive(Debug, Default)]
pub struct Options {
    pub release: bool,
    /// Use this prebuilt executable instead of building the app.
    pub exe: Option<PathBuf>,
}

/// Assembles an installable bundle and returns its path.
pub fn bundle(platform: Platform, app: &App, options: &Options) -> anyhow::Result<PathBuf> {
    match platform {
        Platform::MacOs => macos::bundle(app, options),
        Platform::IosSim => ios_sim::bundle(app, options),
        Platform::Android => android::bundle(app, options),
    }
}

/// Builds, cooks, deploys and launches, forwarding `args` to the app.
pub fn run(platform: Platform, app: &App, options: &Options, args: &[String]) -> anyhow::Result<()> {
    match platform {
        Platform::MacOs => macos::run(app, options, args),
        Platform::IosSim => ios_sim::run(app, options, args),
        Platform::Android => android::run(app, options, args),
    }
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
            &CargoBuild { package: app.package, target, release: options.release, env },
            app.package,
        ),
    }
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
    let text =
        std::fs::read_to_string(&path).map_err(|e| anyhow!("failed to read {}: {e}", path.display()))?;
    Ok((path, text))
}

fn display(path: &Path) -> String {
    path.strip_prefix(workspace::root()).unwrap_or(path).display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_platforms() {
        assert_eq!("macos".parse::<Platform>().unwrap(), Platform::MacOs);
        assert_eq!("ios-sim".parse::<Platform>().unwrap(), Platform::IosSim);
        assert_eq!("android".parse::<Platform>().unwrap(), Platform::Android);
        let err = "ios".parse::<Platform>().unwrap_err().to_string();
        assert_eq!(err, "unknown platform `ios`; expected one of: macos, ios-sim, android");
    }

    #[test]
    fn bundle_names_fit_apple_limits() {
        assert!(SANDBOX.bundle_name.len() <= 15);
        assert!(SANDBOX.bundle_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'));
    }
}
