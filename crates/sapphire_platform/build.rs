//! Links clang's compiler runtime on Apple targets.
//!
//! SDL's Objective-C code checks OS versions with `@available`, which compiles to calls to
//! `__isPlatformVersionAtLeast`. Clang links that from its runtime (`libclang_rt.<os>.a`) into
//! every C program, but rustc links with `-nodefaultlibs`. Rust's std defines the symbol too,
//! so unoptimized builds link anyway; with LTO, though, std's copy is internalized and dropped
//! because no Rust code calls it, and release builds failed with an undefined symbol. Linking
//! the runtime the way clang would fixes every profile: archive members are only pulled in for
//! symbols nothing else defines.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let runtime =
        match (env::var("CARGO_CFG_TARGET_OS").as_deref(), env::var("CARGO_CFG_TARGET_ABI").as_deref()) {
            // Mac Catalyst runs on the macOS runtime.
            (Ok("macos"), _) | (Ok("ios"), Ok("macabi")) => "osx",
            (Ok("ios"), Ok("sim")) => "iossim",
            (Ok("ios"), _) => "ios",
            _ => return,
        };
    // The directory names clang's version, so an Xcode update or switch must rerun this: Cargo
    // reruns when a watched file changes or disappears.
    println!("cargo::rerun-if-env-changed=DEVELOPER_DIR");
    let Some(dir) = clang_runtime_dir() else {
        println!("cargo::warning=can't find clang's runtime directory; release builds may fail to link");
        return;
    };
    let archive = dir.join(format!("libclang_rt.{runtime}.a"));
    println!("cargo::rerun-if-changed={}", archive.display());
    if !archive.is_file() {
        println!("cargo::warning=no libclang_rt.{runtime}.a in {}", dir.display());
        return;
    }
    println!("cargo::rustc-link-search=native={}", dir.display());
    // `-bundle`: the archive is fat (several architectures), so the final link reads it
    // directly instead of rustc copying it into this crate's rlib.
    println!("cargo::rustc-link-lib=static:-bundle=clang_rt.{runtime}");
}

/// `<resource dir>/lib/darwin` of the active Xcode clang.
fn clang_runtime_dir() -> Option<PathBuf> {
    let output = Command::new("xcrun").args(["clang", "-print-resource-dir"]).output().ok()?;
    let dir = String::from_utf8(output.stdout).ok()?;
    output.status.success().then(|| PathBuf::from(dir.trim()).join("lib").join("darwin"))
}
