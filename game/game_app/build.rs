//! Compiles in what the game is (M4 S1): its bundle ID from `platform/identity.toml`, which it tells SDL, and
//! the build `cargo xtask package` made it as, from the environment the package's build runs in
//! (`SAPPHIRE_BUILD_NUMBER` and `SAPPHIRE_BUILD_ID`, xtask's `version` module). A build xtask's `package`
//! didn't make has no number, and a build ID marked `local`. `game_app::build_info` holds them.
//!
//! Windows' resources (the manifest, the icon and VERSIONINFO, M4 S4) go in here too, with the same number.

use std::path::PathBuf;
use std::{env, fs};

const NUMBER: &str = "SAPPHIRE_BUILD_NUMBER";
const ID: &str = "SAPPHIRE_BUILD_ID";

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    // Joined, never formatted: on Windows a canonical root is a `\\?\` path, where `/` isn't a separator.
    let identities = manifest.join("..").join("..").join("platform").join("identity.toml");
    println!("cargo:rerun-if-changed={}", identities.display());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed={NUMBER}");
    println!("cargo:rerun-if-env-changed={ID}");

    let text = fs::read_to_string(&identities).expect("platform/identity.toml");
    let table: toml::Table = toml::from_str(&text).expect("platform/identity.toml is TOML");
    let bundle_id = table
        .get("game_app")
        .and_then(|game| game.get("bundle-id"))
        .and_then(toml::Value::as_str)
        .expect("platform/identity.toml names game_app's bundle-id");

    let version = env::var("CARGO_PKG_VERSION").expect("cargo sets it");
    let number = env::var(NUMBER).ok().map(|number| {
        number.parse::<u32>().unwrap_or_else(|_| panic!("{NUMBER}={number:?} isn't a build number"))
    });
    let id = match (env::var(ID).ok(), number) {
        (Some(id), Some(_)) => id,
        (None, None) => format!("{version}+local"),
        _ => panic!("{NUMBER} and {ID} come together, from `cargo xtask package`"),
    };

    let code = format!(
        "/// The bundle ID: Apple's bundle identifier and Android's application id (`platform/identity.toml`).
pub const BUNDLE_ID: &str = {bundle_id:?};
/// The build number `cargo xtask package` counted, or `None` in a build it didn't make.
pub const BUILD_NUMBER: Option<u32> = {number:?};
/// The version and build number, as the pipeline cache and crash reports name this build.
pub const BUILD_ID: &str = {id:?};
"
    );
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets it")).join("build_info.rs");
    fs::write(out, code).expect("OUT_DIR is writable");
}
