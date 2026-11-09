//! Compiles in the scenario documents and what they name (M3 B2), so a replay still needs no assets and a
//! headless runner on any OS has them, and a new scenario or encounter takes no Rust.
//!
//! Every `content/scenarios/*.scenario.ron` goes in, and so does each `.ron` a scenario names by a path from
//! the repository's root, with its sidecar, if it has one, for the asset ID in it. A move set with a cooked
//! copy in `baked/` takes that rather than its document, since cooking its tables takes the pipeline. So does
//! the `.luau` a kind of enemy names by ID as its `script` (M3 C2), found by its sidecar anywhere under
//! `content/`, so a script reaches a fight from documents alone. `replay::document` reads them.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

/// The strings in a RON document that name a `.ron` file. Comments are skipped, since one may hold anything.
fn named_documents(text: &str) -> impl Iterator<Item = String> + '_ {
    text.lines().flat_map(|line| {
        let code = line.split("//").next().unwrap_or_default();
        code.split('"').skip(1).step_by(2).filter(|string| string.ends_with(".ron")).map(str::to_owned)
    })
}

/// The ID an enemy document names as its `script`, if it names one.
fn script_of(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let code = line.split("//").next().unwrap_or_default().trim_start();
        let rest = code.strip_prefix("script:")?;
        Some(rest.split('"').nth(1)?.to_owned())
    })
}

/// The ID in a sidecar's text.
fn id_in_sidecar(text: &str) -> Option<String> {
    text.lines().find_map(|line| Some(line.trim_start().strip_prefix("id:")?.split('"').nth(1)?.to_owned()))
}

/// Every `.luau` under `directory` that has a sidecar, by the ID in it, as paths from `root`.
fn scripts_by_id(root: &Path, directory: &Path, found: &mut Vec<(String, String)>) {
    let Ok(entries) = fs::read_dir(directory) else { return };
    for entry in entries {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            scripts_by_id(root, &path, found);
        } else if let Some(source) = path.to_str().and_then(|path| path.strip_suffix(".luau.meta")) {
            let text = fs::read_to_string(&path).expect("a sidecar");
            let id = id_in_sidecar(&text).unwrap_or_else(|| panic!("{} has no ID", path.display()));
            let relative =
                Path::new(&format!("{source}.luau")).strip_prefix(root).expect("under the root").to_owned();
            found.push((id, relative.to_str().expect("a UTF-8 path").to_owned()));
        }
    }
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
    // The scripts kinds of enemy name, by ID: what they are is only found by looking.
    let wanted: BTreeSet<String> = named
        .iter()
        .filter(|relative| relative.ends_with(".enemy.ron"))
        .filter_map(|relative| script_of(&fs::read_to_string(root.join(relative)).ok()?))
        .collect();
    if !wanted.is_empty() {
        let mut found = Vec::new();
        scripts_by_id(&root, &root.join("content"), &mut found);
        for id in &wanted {
            let (_, relative) = found.iter().find(|(found, _)| found == id).unwrap_or_else(|| {
                panic!("a kind of enemy names the script {id}, which no `.luau` under content/ is")
            });
            named.insert(relative.clone());
        }
    }
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

    // Every zone's sidecar, by the zone's stem: an exit names the zone it leads to by ID, and a scenario's
    // zone names that zone's pack as a cook does (M3 W4).
    let zones = root.join("content/zones");
    println!("cargo:rerun-if-changed={}", zones.display());
    let mut sidecars: Vec<PathBuf> = fs::read_dir(&zones)
        .expect("content/zones")
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.is_dir())
        .flat_map(|directory| fs::read_dir(directory).expect("a zone's directory"))
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.to_string_lossy().ends_with(".zone.ron.meta"))
        .collect();
    sidecars.sort();
    code.push_str("\npub(super) const ZONE_SIDECARS: &[(&str, &str)] = &[\n");
    for path in &sidecars {
        let file = path.file_name().and_then(|name| name.to_str()).expect("a UTF-8 name");
        let stem = file.strip_suffix(".zone.ron.meta").expect("filtered");
        writeln!(code, "    ({stem:?}, include_str!({:?})),", path.display().to_string()).expect("a string");
        println!("cargo:rerun-if-changed={}", path.display());
    }
    code.push_str("];\n");

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets it"));
    fs::write(out.join("scenario_documents.rs"), code).expect("OUT_DIR is writable");
}
