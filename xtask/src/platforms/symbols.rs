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
//! ELF keeps the DWARF whole rather than in a `.dwp`, because what reads symbols (`sapphire_symbols`' `.sym`
//! writer, Play's native symbols) wants an ordinary ELF. The build ID is explicit because no linker owes one,
//! and lld's for a bare `--build-id` (which distributions' gcc passes) is an 8-byte hash where a debug ID
//! takes 16.
//!
//! **Paths (M4 C2).** A build records where its sources were: in panic locations in the binary and in every
//! line of its symbols. A shipping build remaps this machine's directories to fixed names, rustc's paths
//! with `--remap-path-prefix` and C and C++'s with `-ffile-prefix-map` (and `-fdebug-prefix-map`, the only
//! one GCC 10 hands its assembler, so ring's `.S` files' compile directories are remapped too): the
//! workspace to [`WORKSPACE_PREFIX`], Cargo's home (the registry's crates) to [`CARGO_HOME_PREFIX`], a
//! target directory outside the workspace to `/target`, and Android's NDK to `/ndk`. MSVC has no map, so
//! its `/d1trimfile` cuts the directories from the C and C++ `__FILE__`s the executable holds (Luau's
//! asserts name theirs), and its PDB names this machine (no flag of link.exe's remaps the directory it ran
//! in or its objects' paths): a Windows package's scan reads only what ships. [`scan`] proves that no package carries them or
//! the builder's home directory, and `cargo xtask symbolicate` maps the names back ([`symbolicate`]).

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, ensure};

use super::package::Target;
use crate::workspace::{self, BuildProfile, CargoBuild};

/// What a shipping build calls the workspace in its paths.
pub const WORKSPACE_PREFIX: &str = "/sapphire";
/// What a shipping build calls Cargo's home: the registry's crates are under `/cargo/registry/src/`.
pub const CARGO_HOME_PREFIX: &str = "/cargo";
/// What Android's shipping build calls the NDK, whose headers and runtime its C and C++ name.
pub const NDK_PREFIX: &str = "/ndk";

/// A directory of this machine's, and the name a shipping build gives it in every path it records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remap {
    pub local: PathBuf,
    pub shipped: &'static str,
}

/// The directories every shipping build's sources are in: the workspace, Cargo's home, and the target
/// directory if it's outside the workspace. Android's build adds its NDK.
pub fn remaps() -> anyhow::Result<Vec<Remap>> {
    let root = workspace::root();
    let mut remaps = vec![
        Remap { local: root.clone(), shipped: WORKSPACE_PREFIX },
        Remap { local: cargo_home()?, shipped: CARGO_HOME_PREFIX },
    ];
    if let Some(target) = std::env::var_os("CARGO_TARGET_DIR").filter(|dir| !dir.is_empty()) {
        let target = root.join(target);
        if !target.starts_with(&root) {
            remaps.push(Remap { local: target, shipped: "/target" });
        }
    }
    Ok(remaps)
}

/// `CARGO_HOME`, else `.cargo` in the user's home, as Cargo finds it.
fn cargo_home() -> anyhow::Result<PathBuf> {
    if let Some(home) = std::env::var_os("CARGO_HOME").filter(|home| !home.is_empty()) {
        return Ok(PathBuf::from(home));
    }
    Ok(user_home().context("no CARGO_HOME, HOME or USERPROFILE: where is Cargo's home?")?.join(".cargo"))
}

fn user_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// The Rust target triple a package for `target` builds for: the host's, for a desktop, which builds
/// natively.
fn triple(target: Target) -> String {
    let arch = std::env::consts::ARCH;
    match target {
        Target::Ios => "aarch64-apple-ios".into(),
        Target::IosSim => "aarch64-apple-ios-sim".into(),
        Target::Android => super::android::TARGET.into(),
        Target::MacOs => format!("{arch}-apple-darwin"),
        Target::Windows => format!("{arch}-pc-windows-msvc"),
        Target::Linux => format!("{arch}-unknown-linux-gnu"),
    }
}

/// What `target`'s shipping build is given, as Cargo's environment: the profile's settings it overrides,
/// since a profile can't differ by target, and the flags that remap `remaps` in every path rustc and the C
/// and C++ compilers record. The flags go in the target's own variables, which Cargo and cc-rs add to
/// `.cargo/config.toml`'s and the plain `CFLAGS` rather than replace.
///
/// Windows' links the C runtime statically (M4 X4), so the package needs no Visual C++ redistributable on
/// the player's machine: `+crt-static` for rustc, which cc-rs follows with `/MT`, and
/// [`STATIC_CRT_TOOLCHAIN`] for CMake's builds (SDL's), whose runtime CMake chooses by its own variable.
/// Only shipping builds do: a dev build, the tools and CI's tests keep the DLL's runtime.
pub fn link_env(target: Target, remaps: &[Remap]) -> anyhow::Result<Vec<(String, OsString)>> {
    let mut env: Vec<(String, OsString)> = match target {
        Target::Ios | Target::IosSim | Target::MacOs => {
            vec![("CARGO_PROFILE_SHIPPING_STRIP".into(), "symbols".into())]
        }
        Target::Android | Target::Linux => {
            vec![("CARGO_PROFILE_SHIPPING_SPLIT_DEBUGINFO".into(), "off".into())]
        }
        Target::Windows => Vec::new(),
    };
    let mut rustflags = Vec::new();
    let mut cflags = Vec::new();
    let msvc = target == Target::Windows;
    if msvc {
        rustflags.push("-Ctarget-feature=+crt-static".to_owned());
    }
    for remap in remaps {
        let local = remap.local.to_str().with_context(|| format!("{} isn't UTF-8", remap.local.display()))?;
        // Cargo and cc-rs split these variables at whitespace.
        ensure!(
            !local.chars().any(char::is_whitespace),
            "can't remap {local} in a shipping build's paths: it has a space, and the compilers' flags are \
             split at spaces"
        );
        rustflags.push(format!("--remap-path-prefix={local}={}", remap.shipped));
        if msvc {
            cflags.push(format!("/d1trimfile:{local}"));
        } else {
            // `-ffile-prefix-map` covers `__FILE__` and the debug information, but the Steam Linux
            // Runtime's GCC 10 doesn't pass it on to the assembler as `--debug-prefix-map`, and an assembly
            // file's DWARF then names the directory it was assembled in (ring's, in Cargo's registry).
            cflags.push(format!("-ffile-prefix-map={local}={}", remap.shipped));
            cflags.push(format!("-fdebug-prefix-map={local}={}", remap.shipped));
        }
    }
    let triple = triple(target);
    let upper = triple.to_uppercase().replace('-', "_");
    env.push((format!("CARGO_TARGET_{upper}_RUSTFLAGS"), rustflags.join(" ").into()));
    let lower = triple.replace('-', "_");
    env.push((format!("CFLAGS_{lower}"), cflags.join(" ").into()));
    env.push((format!("CXXFLAGS_{lower}"), cflags.join(" ").into()));
    if msvc {
        // cmake-rs reads the target's own toolchain variable before the plain one.
        env.push((
            format!("CMAKE_TOOLCHAIN_FILE_{lower}"),
            workspace::root().join(STATIC_CRT_TOOLCHAIN).into(),
        ));
    }
    Ok(env)
}

/// The CMake toolchain file that has CMake's builds link the C runtime statically in a shipping Windows build.
pub const STATIC_CRT_TOOLCHAIN: &str = "platform/windows/static_crt.cmake";

/// Fails if any file in `paths` (files, or directories read whole) holds one of the `local` directories of
/// `remaps` or the builder's home directory, with either kind of slash: what no shipping build may carry.
pub fn scan(paths: &[&Path], remaps: &[Remap]) -> anyhow::Result<()> {
    let mut command = Command::new(checker()?);
    command.arg("scan");
    let home = user_home();
    for local in remaps.iter().map(|remap| remap.local.as_path()).chain(home.as_deref()) {
        command.arg("--forbid").arg(local);
    }
    command.args(paths);
    workspace::run(&mut command)
}

/// Writes the shipped prefixes of `remaps` into each dSYM's relocation maps under `roots`: `dsymutil` records
/// the path of the binary it read in `Contents/Resources/Relocations/<arch>/*.yml` (`binary-path`), the one
/// place a remapped link still names this machine's directories. Nothing reads that path back.
pub fn remap_dsym_relocations(roots: &[&Path], remaps: &[Remap]) -> anyhow::Result<()> {
    let mut by_length: Vec<&Remap> = remaps.iter().collect();
    by_length.sort_by_key(|remap| std::cmp::Reverse(remap.local.as_os_str().len()));
    let mut maps = Vec::new();
    for root in roots {
        relocation_maps(root, false, &mut maps)?;
    }
    for map in maps {
        let text = fs::read_to_string(&map).with_context(|| format!("failed to read {}", map.display()))?;
        let mut remapped = text.clone();
        for remap in &by_length {
            let local = remap.local.to_str().context("a local path isn't UTF-8")?;
            remapped = remapped.replace(local, remap.shipped);
        }
        if remapped != text {
            fs::write(&map, remapped).with_context(|| format!("failed to write {}", map.display()))?;
        }
    }
    Ok(())
}

/// The `.yml` files under a `Relocations` directory of a dSYM in `dir`, `in_dsym` once inside one.
fn relocation_maps(dir: &Path, in_dsym: bool, maps: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    let Ok(entries) = fs::read_dir(dir) else { return Ok(()) };
    for entry in entries {
        let entry = entry.with_context(|| format!("failed to list {}", dir.display()))?;
        let path = entry.path();
        let kind = entry.file_type().with_context(|| format!("failed to read {}", path.display()))?;
        if kind.is_dir() {
            let dsym = in_dsym || path.extension().is_some_and(|extension| extension == "dSYM");
            relocation_maps(&path, dsym, maps)?;
        } else if kind.is_file()
            && in_dsym
            && path.extension().is_some_and(|extension| extension == "yml")
            && path.components().any(|part| part.as_os_str() == "Relocations")
        {
            maps.push(path);
        }
    }
    Ok(())
}

/// `cargo xtask symbolicate`: reads each of `reports` (a minidump, a panic report, an Android signal record
/// or tombstone) with the symbols of `packages`, and prints its threads' frames with their functions, files
/// and lines, the shipped paths mapped back to this machine's. `store` keeps the `.sym` files it makes.
pub fn symbolicate(packages: &[PathBuf], store: &Path, reports: &[PathBuf]) -> anyhow::Result<()> {
    let mut command = Command::new(checker()?);
    command.arg("symbolicate").arg("--store").arg(store);
    for package in packages {
        command.arg("--package").arg(package);
    }
    for remap in remaps()? {
        let mut map = OsString::from(remap.shipped);
        map.push("=");
        map.push(&remap.local);
        command.arg("--map").arg(map);
    }
    command.args(reports);
    workspace::run(&mut command)
}

/// `cargo xtask symbols`: writes the `.sym` of every module `packages` shipped into `store`, the whole
/// Breakpad store for them, where `symbolicate` writes only the modules a crash names. The nightly packages
/// keep it beside each package (M4 I2).
pub fn write_store(packages: &[PathBuf], store: &Path) -> anyhow::Result<()> {
    let mut command = Command::new(checker()?);
    command.arg("store").arg("--store").arg(store);
    for package in packages {
        command.arg("--package").arg(package);
    }
    workspace::run(&mut command)
}

/// Where `cargo xtask symbolicate` and `symbols` keep the `.sym` files they make, as a Breakpad store:
/// `target/symbols/<debug file>/<debug ID>/<name>.sym`.
pub fn default_store() -> PathBuf {
    workspace::output_dir().join("symbols")
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
/// `entry` is its path there, and it's unpacked into `scratch` to be read. It stays there for the caller
/// to read too, until the caller removes `scratch`.
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
    let parts: Option<Vec<&str>> = inside.iter().map(OsStr::to_str).collect();
    Ok(parts.with_context(|| format!("{} isn't UTF-8", inside.display()))?.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Apple strips at the link, after `dsymutil`; ELF keeps its DWARF whole, for a stripped copy;
    /// Windows' link already leaves its debug information in the PDB alone.
    #[test]
    fn each_target_links_for_its_symbols() {
        let first = |target| {
            let (name, value) = link_env(target, &[]).unwrap().swap_remove(0);
            format!("{name}={}", value.display())
        };
        assert_eq!(first(Target::MacOs), "CARGO_PROFILE_SHIPPING_STRIP=symbols");
        assert_eq!(first(Target::Ios), first(Target::IosSim));
        assert_eq!(first(Target::Android), "CARGO_PROFILE_SHIPPING_SPLIT_DEBUGINFO=off");
        assert_eq!(first(Target::Linux), first(Target::Android));
        assert!(first(Target::Windows).starts_with("CARGO_TARGET_"), "{}", first(Target::Windows));
    }

    /// Each target's own variables carry the remaps, so `.cargo/config.toml`'s flags for it still apply.
    /// MSVC, which has no `-ffile-prefix-map`, trims the directories from `__FILE__` instead.
    #[test]
    fn shipping_builds_remap_local_paths() {
        let remaps = [
            Remap { local: PathBuf::from("/work/engine"), shipped: WORKSPACE_PREFIX },
            Remap { local: PathBuf::from("/u/ana/.cargo"), shipped: CARGO_HOME_PREFIX },
        ];
        let env = link_env(Target::Android, &remaps).unwrap();
        let get = |name: &str| env.iter().find(|(key, _)| key == name).and_then(|(_, value)| value.to_str());
        assert_eq!(
            get("CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS"),
            Some("--remap-path-prefix=/work/engine=/sapphire --remap-path-prefix=/u/ana/.cargo=/cargo")
        );
        assert_eq!(
            get("CXXFLAGS_aarch64_linux_android"),
            Some(
                "-ffile-prefix-map=/work/engine=/sapphire -fdebug-prefix-map=/work/engine=/sapphire \
                 -ffile-prefix-map=/u/ana/.cargo=/cargo -fdebug-prefix-map=/u/ana/.cargo=/cargo"
            )
        );
        let ios = link_env(Target::IosSim, &remaps).unwrap();
        assert!(ios.iter().any(|(name, _)| name == "CARGO_TARGET_AARCH64_APPLE_IOS_SIM_RUSTFLAGS"));
        let windows = link_env(Target::Windows, &remaps).unwrap();
        let windows_cxx = windows
            .iter()
            .find(|(name, _)| name.starts_with("CXXFLAGS_") && name.ends_with("_pc_windows_msvc"));
        assert_eq!(
            windows_cxx.and_then(|(_, value)| value.to_str()),
            Some("/d1trimfile:/work/engine /d1trimfile:/u/ana/.cargo")
        );

        let windows_rust = windows.iter().find(|(name, _)| name.ends_with("_PC_WINDOWS_MSVC_RUSTFLAGS"));
        assert_eq!(
            windows_rust.and_then(|(_, value)| value.to_str()),
            Some(
                "-Ctarget-feature=+crt-static --remap-path-prefix=/work/engine=/sapphire \
                 --remap-path-prefix=/u/ana/.cargo=/cargo"
            ),
            "the C runtime linked in, so no redistributable"
        );
        let toolchain = windows.iter().find(|(name, _)| name.starts_with("CMAKE_TOOLCHAIN_FILE_"));
        let toolchain =
            toolchain.map(|(_, value)| PathBuf::from(value)).expect("CMake's builds are told too");
        assert_eq!(toolchain, workspace::root().join(STATIC_CRT_TOOLCHAIN));
        let text = fs::read_to_string(&toolchain).unwrap();
        assert!(text.contains(r#"set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")"#));
        assert!(env.iter().all(|(name, _)| !name.starts_with("CMAKE_")), "only Windows'");
        assert!(!get("CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS").unwrap().contains("crt-static"));

        let spaced = [Remap { local: PathBuf::from("/my work"), shipped: WORKSPACE_PREFIX }];
        assert!(link_env(Target::MacOs, &spaced).is_err());
        let here = super::remaps().unwrap();
        assert_eq!(here[0], Remap { local: workspace::root(), shipped: WORKSPACE_PREFIX });
        assert_eq!(here[1].shipped, CARGO_HOME_PREFIX);
    }

    #[test]
    fn package_paths_are_relative_with_slashes() {
        let package = Path::new("out").join("macos");
        let shipped = package.join("SapphireGame.app").join("Contents").join("MacOS").join("game_app");
        assert_eq!(relative(&package, &shipped).unwrap(), "SapphireGame.app/Contents/MacOS/game_app");
        assert!(relative(&package, Path::new("elsewhere")).is_err());
    }
}
