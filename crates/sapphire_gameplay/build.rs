//! Compiles in the scenario documents and what they name (M3 B2), so a replay still needs no assets and a
//! headless runner on any OS has them, and a new scenario or encounter takes no Rust.
//!
//! Every `content/scenarios/*.scenario.ron` goes in, and so does each `.ron` a scenario names by a path from
//! the repository's root, with its sidecar, if it has one, for the asset ID in it. A move set with a cooked
//! copy in `baked/` takes that rather than its document, since cooking its tables takes the pipeline.
//! `replay::document` reads them.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::{env, fs};

/// The strings in a RON document that name a `.ron` file. Comments are skipped, since one may hold anything.
fn named_documents(text: &str) -> impl Iterator<Item = String> + '_ {
    text.lines().flat_map(|line| {
        let code = line.split("//").next().unwrap_or_default();
        code.split('"').skip(1).step_by(2).filter(|string| string.ends_with(".ron")).map(str::to_owned)
    })
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let root = manifest.join("../..").canonicalize().expect("the repository's root");
    let directory = root.join("content/scenarios");
    println!("cargo:rerun-if-changed={}", directory.display());
    println!("cargo:rerun-if-changed=build.rs");

    let mut scenarios: Vec<PathBuf> = fs::read_dir(&directory)
        .expect("content/scenarios")
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.to_string_lossy().ends_with(".scenario.ron"))
        .collect();
    scenarios.sort();

    let mut named = BTreeSet::new();
    let mut code = String::from("pub(super) const SCENARIO_DOCUMENTS: &[(&str, &str)] = &[\n");
    for path in &scenarios {
        let text = fs::read_to_string(path).expect("a scenario document");
        named.extend(named_documents(&text));
        let file = path.file_name().and_then(|name| name.to_str()).expect("a UTF-8 name");
        let stem = file.strip_suffix(".scenario.ron").expect("filtered");
        writeln!(code, "    ({stem:?}, include_str!({:?})),", path.display().to_string()).expect("a string");
        println!("cargo:rerun-if-changed={}", path.display());
    }
    code.push_str("];\n\npub(super) const NAMED_DOCUMENTS: &[(&str, &str)] = &[\n");
    let mut baked = BTreeSet::new();
    let embed = |relative: &str, code: &mut String| {
        let path = root.join(relative);
        assert!(path.is_file(), "a scenario names {relative}, which isn't there");
        writeln!(code, "    ({relative:?}, include_str!({:?})),", path.display().to_string())
            .expect("a string");
        println!("cargo:rerun-if-changed={}", path.display());
    };
    for relative in &named {
        embed(relative, &mut code);
        // `root` is canonical, which on Windows is a `\\?\` path, where `/` isn't a separator: so join.
        if root.join(format!("{relative}.meta")).is_file() {
            embed(&format!("{relative}.meta"), &mut code);
        }
        let stem = relative.rsplit('/').next().and_then(|file| file.strip_suffix(".moves.ron"));
        if let Some(stem) = stem {
            let cooked = manifest.join("baked").join(format!("{stem}.smoves"));
            if cooked.is_file() {
                baked.insert((stem.to_owned(), cooked));
            }
        }
    }
    code.push_str("];\n\npub(super) const BAKED_MOVES: &[(&str, &[u8])] = &[\n");
    for (stem, cooked) in &baked {
        writeln!(code, "    ({stem:?}, include_bytes!({:?})),", cooked.display().to_string())
            .expect("a string");
        println!("cargo:rerun-if-changed={}", cooked.display());
    }
    code.push_str("];\n");

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets it"));
    fs::write(out.join("scenario_documents.rs"), code).expect("OUT_DIR is writable");
}
