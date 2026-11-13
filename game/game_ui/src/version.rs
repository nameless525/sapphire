//! What a build says it is (Sapphire 0.1.0): the title's version label and `--version`'s line.
//!
//! The version is the workspace's (Cargo.toml's `[workspace.package]`), and the build number the one
//! `cargo xtask package` counted, which only a packaged build has (`game_app`'s `build_info`). The title
//! doesn't read either: the app hands it the label ([`MenuSetup::version`](crate::menus::MenuSetup::version)),
//! so the sandbox can pin it and no golden moves when the version does.

/// A build's version and, in a packaged build, its number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildVersion<'a> {
    /// The workspace's version, `0.1.0`.
    pub version: &'a str,
    /// The build number `cargo xtask package` counted, or `None` in a build it didn't make.
    pub build: Option<u32>,
}

impl BuildVersion<'_> {
    /// The title's label: `v0.1.0`, or `v0.1.0 (1234)` in a packaged build. Its digits are the same in every
    /// language, so they're never translated.
    #[must_use]
    pub fn label(&self) -> String {
        match self.build {
            Some(build) => format!("v{} ({build})", self.version),
            None => format!("v{}", self.version),
        }
    }

    /// `--version`'s one line for the app called `name`: `Sapphire 0.1.0`, or `Sapphire 0.1.0 (1234)` in a
    /// packaged build.
    #[must_use]
    pub fn line(&self, name: &str) -> String {
        match self.build {
            Some(build) => format!("{name} {} ({build})", self.version),
            None => format!("{name} {}", self.version),
        }
    }
}

/// Whether `arg` asks for `--version`, which an app answers with [`BuildVersion::line`] and exits before any
/// platform, window or GPU work.
#[must_use]
pub fn is_version_flag(arg: &str) -> bool {
    arg == "--version"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_local_build_says_its_version_and_a_packaged_one_its_number_too() {
        let local = BuildVersion { version: "0.1.0", build: None };
        assert_eq!(local.label(), "v0.1.0");
        assert_eq!(local.line("Sapphire"), "Sapphire 0.1.0");
        let packaged = BuildVersion { version: "0.1.0", build: Some(1234) };
        assert_eq!(packaged.label(), "v0.1.0 (1234)");
        assert_eq!(packaged.line("Sapphire"), "Sapphire 0.1.0 (1234)");
        assert_eq!(packaged.line("Sapphire Sandbox"), "Sapphire Sandbox 0.1.0 (1234)");
    }

    #[test]
    fn only_the_long_flag_asks() {
        assert!(is_version_flag("--version"));
        for other in ["-V", "-v", "--versions", "--ui-version", "version"] {
            assert!(!is_version_flag(other), "{other}");
        }
    }
}
