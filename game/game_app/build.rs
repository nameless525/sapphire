//! Compiles in what the game is (M4 S1): its bundle ID from `platform/identity.toml`, which it tells SDL, and
//! the build `cargo xtask package` made it as, from the environment the package's build runs in
//! (`SAPPHIRE_BUILD_NUMBER` and `SAPPHIRE_BUILD_ID`, xtask's `version` module). A build xtask's `package`
//! didn't make has no number, and a build ID marked `local`. `game_app::build_info` holds them.
//!
//! For Windows (M4 S4) it links the executable's resources: the application manifest
//! (`platform/windows/game_app.manifest`), VERSIONINFO with the same version and number, and the icon a
//! package's build names in `SAPPHIRE_WINDOWS_ICON`. A package's build (one with a number) is also a GUI
//! program, the `sapphire_packaged` cfg, so Steam opens no console beside it; other builds keep theirs for
//! the log. The target is read from Cargo's environment, never `#[cfg(target_os)]`, which would be the host's.

use std::path::{Path, PathBuf};
use std::{env, fs};

const NUMBER: &str = "SAPPHIRE_BUILD_NUMBER";
const ID: &str = "SAPPHIRE_BUILD_ID";
const WINDOWS_ICON: &str = "SAPPHIRE_WINDOWS_ICON";

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    // Joined, never formatted: on Windows a canonical root is a `\\?\` path, where `/` isn't a separator.
    let platform = manifest.join("..").join("..").join("platform");
    let identities = platform.join("identity.toml");
    println!("cargo:rerun-if-changed={}", identities.display());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed={NUMBER}");
    println!("cargo:rerun-if-env-changed={ID}");
    println!("cargo:rustc-check-cfg=cfg(sapphire_packaged)");

    let text = fs::read_to_string(&identities).expect("platform/identity.toml");
    let table: toml::Table = toml::from_str(&text).expect("platform/identity.toml is TOML");
    let game = table.get("game_app").expect("platform/identity.toml has game_app");
    let bundle_id = game
        .get("bundle-id")
        .and_then(toml::Value::as_str)
        .expect("platform/identity.toml names game_app's bundle-id");
    let display_name = game
        .get("display-name")
        .and_then(|names| names.get("en"))
        .and_then(toml::Value::as_str)
        .expect("platform/identity.toml names game_app's English display name");

    let version = env::var("CARGO_PKG_VERSION").expect("cargo sets it");
    let number = env::var(NUMBER).ok().map(|number| {
        number.parse::<u32>().unwrap_or_else(|_| panic!("{NUMBER}={number:?} isn't a build number"))
    });
    let id = match (env::var(ID).ok(), number) {
        (Some(id), Some(_)) => id,
        (None, None) => format!("{version}+local"),
        _ => panic!("{NUMBER} and {ID} come together, from `cargo xtask package`"),
    };
    if number.is_some() {
        println!("cargo:rustc-cfg=sapphire_packaged");
    }

    let code = format!(
        "/// The bundle ID: Apple's bundle identifier and Android's application id (`platform/identity.toml`).
pub const BUNDLE_ID: &str = {bundle_id:?};
/// The build number `cargo xtask package` counted, or `None` in a build it didn't make.
pub const BUILD_NUMBER: Option<u32> = {number:?};
/// The version and build number, as the pipeline cache and crash reports name this build.
pub const BUILD_ID: &str = {id:?};
"
    );
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets it"));
    fs::write(out.join("build_info.rs"), code).expect("OUT_DIR is writable");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let resources = Resources {
            manifest: platform.join("windows").join("game_app.manifest"),
            icon: env::var_os(WINDOWS_ICON).map(PathBuf::from),
            name: display_name,
            version: &version,
            number: number.unwrap_or(0),
            id: &id,
        };
        link_windows_resources(&resources, &out);
    }
}

/// What the executable's resources say.
struct Resources<'a> {
    manifest: PathBuf,
    icon: Option<PathBuf>,
    /// The English display name: the file's description and product name.
    name: &'a str,
    version: &'a str,
    /// The build number, 0 in a build `package` didn't make.
    number: u32,
    id: &'a str,
}

/// Writes the resource script and links it into the executable (`embed-resource`, with the Windows SDK's
/// `rc.exe` on a Windows host, or `llvm-rc` cross-compiling). Without a resource compiler the build fails,
/// rather than make an executable with no manifest.
fn link_windows_resources(resources: &Resources<'_>, out: &Path) {
    println!("cargo:rerun-if-changed={}", resources.manifest.display());
    println!("cargo:rerun-if-env-changed={WINDOWS_ICON}");
    if let Some(icon) = &resources.icon {
        println!("cargo:rerun-if-changed={}", icon.display());
    }
    let script = out.join("game_app.rc");
    fs::write(&script, resource_script(resources)).expect("OUT_DIR is writable");
    embed_resource::compile(&script, embed_resource::NONE)
        .manifest_required()
        .expect("the Windows resources compile (rc.exe from the Windows SDK, or llvm-rc)");
}

/// The resource script: the manifest as resource 1 of type 24 (`RT_MANIFEST`, read at process creation),
/// the icon as 1 (the one Explorer and the taskbar take, the lowest ID), and VERSIONINFO. Its numbers are
/// written out, since there's no `winver.h` to include when cross-compiling.
fn resource_script(resources: &Resources<'_>) -> String {
    let [major, minor, patch] = version_parts(resources.version);
    let number = u16::try_from(resources.number)
        .unwrap_or_else(|_| panic!("build {} doesn't fit VERSIONINFO's 16 bits", resources.number));
    let four = format!("{major},{minor},{patch},{number}");
    let dotted = format!("{major}.{minor}.{patch}.{number}");
    let mut script = String::new();
    script.push_str(&format!("1 24 \"{}\"\n", rc_path(&resources.manifest)));
    if let Some(icon) = &resources.icon {
        script.push_str(&format!("1 ICON \"{}\"\n", rc_path(icon)));
    }
    // FILEOS VOS_NT_WINDOWS32, FILETYPE VFT_APP; US English in Unicode (0x409, 1200), whose string table is
    // "040904B0".
    script.push_str(&format!(
        "1 VERSIONINFO
FILEVERSION {four}
PRODUCTVERSION {four}
FILEFLAGSMASK 0x3F
FILEFLAGS 0x0
FILEOS 0x40004
FILETYPE 0x1
FILESUBTYPE 0x0
BEGIN
  BLOCK \"StringFileInfo\"
  BEGIN
    BLOCK \"040904B0\"
    BEGIN
      VALUE \"FileDescription\", \"{name}\"
      VALUE \"FileVersion\", \"{dotted}\"
      VALUE \"InternalName\", \"game_app\"
      VALUE \"OriginalFilename\", \"game_app.exe\"
      VALUE \"ProductName\", \"{name}\"
      VALUE \"ProductVersion\", \"{id}\"
    END
  END
  BLOCK \"VarFileInfo\"
  BEGIN
    VALUE \"Translation\", 0x409, 1200
  END
END
",
        name = rc_text(resources.name),
        id = rc_text(resources.id),
    ));
    script
}

/// The workspace version's three numbers, each in VERSIONINFO's 16 bits.
fn version_parts(version: &str) -> [u16; 3] {
    let core = version.split(['-', '+']).next().unwrap_or(version);
    let parts: Vec<u16> = core
        .split('.')
        .map(|part| part.parse().unwrap_or_else(|_| panic!("version {version:?} isn't three 16-bit numbers")))
        .collect();
    parts.try_into().unwrap_or_else(|_| panic!("version {version:?} isn't three numbers"))
}

/// A path in a resource script's string, where a backslash escapes.
fn rc_path(path: &Path) -> String {
    path.to_str().expect("the workspace's paths are UTF-8").replace('\\', "\\\\")
}

/// Text in a resource script's string: ASCII, since the script has no code page, with quotes doubled.
fn rc_text(text: &str) -> String {
    assert!(text.is_ascii(), "{text:?} goes into VERSIONINFO, which this script writes as ASCII");
    text.replace('"', "\"\"")
}
