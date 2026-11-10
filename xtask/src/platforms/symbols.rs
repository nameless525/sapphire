//! Symbols a package keeps (M4 C1): what each target's link is asked for, and the check that every
//! binary a package ships is stripped and carries its symbols' ID, which `sapphire_symbols` makes.
//!
//! | Target | The link | Shipped | Symbols |
//! |---|---|---|---|
//! | macOS, iOS | `split-debuginfo = "packed"` (the profile's) runs `dsymutil` at the link, then `strip = "symbols"`; ld64 writes `LC_UUID` | the executable in the bundle | `<exe>.dSYM` |
//! | Windows | `/DEBUG` writes the PDB and names it in the image's CodeView record (rustc's `/PDBALTPATH:%_PDB%`: its name only) | `<exe>.exe`, which never holds debug information | `<exe>.pdb` |
//! | Linux | `split-debuginfo = "off"`, so the DWARF stays whole in the binary; `--build-id=sha1` (`.cargo/config.toml`) | a copy `strip --strip-all` made | the unstripped executable |
//! | Android | as Linux | the library in the AAB, which AGP strips | the unstripped library |
//!
//! ELF keeps the DWARF whole rather than in a `.dwp`, because what reads symbols (Breakpad's `dump_syms`,
//! Play's native symbols) wants an ordinary ELF. The build ID is explicit because no linker owes one, and
//! lld's for a bare `--build-id` (which distributions' gcc passes) is an 8-byte hash where a debug ID
//! takes 16.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context as _;

use super::package::Target;
use crate::workspace::{self, BuildProfile, CargoBuild};

/// The profile's settings `target`'s shipping build overrides, as Cargo's environment, since a profile
/// can't differ by target.
pub fn link_env(target: Target) -> Vec<(&'static str, OsString)> {
    match target {
        Target::Ios | Target::IosSim | Target::MacOs => {
            vec![("CARGO_PROFILE_SHIPPING_STRIP", "symbols".into())]
        }
        Target::Android | Target::Linux => vec![("CARGO_PROFILE_SHIPPING_SPLIT_DEBUGINFO", "off".into())],
        Target::Windows => Vec::new(),
    }
}

/// Strips an ELF executable a package ships, in place: its symbol table and DWARF go, its build ID
/// stays.
pub fn strip_elf(path: &Path) -> anyhow::Result<()> {
    workspace::run(Command::new("strip").arg("--strip-all").arg(path))
}

/// Checks `shipped` against `symbols`, both inside `package`, with `sapphire_symbols`, and returns the
/// module they make for `package.json`: its format, arch, debug file, debug ID and code ID, and both
/// paths relative to `package`.
pub fn check(package: &Path, shipped: &Path, symbols: &Path) -> anyhow::Result<serde_json::Value> {
    let shipped_name = relative(package, shipped)?;
    let mut module = run_checker(shipped, symbols, &shipped_name)?;
    module.insert("shipped".into(), shipped_name.into());
    module.insert("symbols".into(), relative(package, symbols)?.into());
    Ok(module.into())
}

/// As [`check`], for a binary that ships inside an archive in `package` (Android's library, in the AAB):
/// `entry` is its path there, and it's unpacked into `scratch` to be read.
pub fn check_in_archive(
    package: &Path,
    archive: &Path,
    entry: &str,
    symbols: &Path,
    scratch: &Path,
) -> anyhow::Result<serde_json::Value> {
    workspace::remove_dir_if_exists(scratch)?;
    workspace::run(Command::new("unzip").arg("-q").arg(archive).arg(entry).arg("-d").arg(scratch))?;
    let archive_name = relative(package, archive)?;
    let mut module = run_checker(&scratch.join(entry), symbols, &format!("{archive_name}'s {entry}"))?;
    workspace::remove_dir_if_exists(scratch)?;
    module.insert("shipped".into(), archive_name.into());
    module.insert("entry".into(), entry.into());
    module.insert("symbols".into(), relative(package, symbols)?.into());
    Ok(module.into())
}

/// Runs `sapphire_symbols check`, and reads the module it prints.
fn run_checker(
    shipped: &Path,
    symbols: &Path,
    name: &str,
) -> anyhow::Result<serde_json::Map<String, serde_json::Value>> {
    let checker = checker()?;
    let line = workspace::output(
        Command::new(&checker).arg("check").arg("--shipped").arg(shipped).arg("--symbols").arg(symbols),
    )?;
    let module: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(line.trim()).with_context(|| format!("`sapphire_symbols` printed {line:?}"))?;
    let field = |key: &str| module.get(key).and_then(|value| value.as_str()).unwrap_or_default().to_owned();
    eprintln!("{name} is stripped and matches its symbols: {} {}", field("debug_file"), field("debug_id"));
    Ok(module)
}

/// Builds `sapphire_symbols` in the tools profile and returns its executable.
fn checker() -> anyhow::Result<PathBuf> {
    workspace::build_binary(
        &CargoBuild {
            package: "sapphire_symbols",
            target: None,
            profile: BuildProfile::Tools,
            features: &[],
            env: &[],
        },
        "sapphire_symbols",
    )
}

/// `path` inside `package`, with `/` between its parts on every host.
fn relative(package: &Path, path: &Path) -> anyhow::Result<String> {
    let inside = path
        .strip_prefix(package)
        .with_context(|| format!("{} isn't inside {}", path.display(), package.display()))?;
    let parts: Option<Vec<&str>> = inside.iter().map(|part| part.to_str()).collect();
    Ok(parts.with_context(|| format!("{} isn't UTF-8", inside.display()))?.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Apple strips at the link, after `dsymutil`; ELF keeps its DWARF whole, for a stripped copy;
    /// Windows' link already leaves its debug information in the PDB alone.
    #[test]
    fn each_target_links_for_its_symbols() {
        let names = |target| {
            link_env(target)
                .into_iter()
                .map(|(name, value)| format!("{name}={}", value.display()))
                .collect::<Vec<_>>()
        };
        assert_eq!(names(Target::MacOs), ["CARGO_PROFILE_SHIPPING_STRIP=symbols"]);
        assert_eq!(names(Target::Ios), names(Target::IosSim));
        assert_eq!(names(Target::Android), ["CARGO_PROFILE_SHIPPING_SPLIT_DEBUGINFO=off"]);
        assert_eq!(names(Target::Linux), names(Target::Android));
        assert!(names(Target::Windows).is_empty());
    }

    #[test]
    fn package_paths_are_relative_with_slashes() {
        let package = Path::new("out").join("macos");
        let shipped = package.join("SapphireGame.app").join("Contents").join("MacOS").join("game_app");
        assert_eq!(relative(&package, &shipped).unwrap(), "SapphireGame.app/Contents/MacOS/game_app");
        assert!(relative(&package, Path::new("elsewhere")).is_err());
    }
}
