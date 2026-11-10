//! Compiles in the scenario documents and what they name (M3 B2), so a replay still needs no assets and a
//! headless runner on any OS has them, and a new scenario or encounter takes no Rust.
//!
//! Every `content/scenarios/*.scenario.ron` goes in, and so does each `.ron` a scenario names by a path from
//! the repository's root, with its sidecar, if it has one, for the asset ID in it. A move set with a cooked
//! copy in `baked/` takes that rather than its document, since cooking its tables takes the pipeline. So does
//! every kind of enemy under `content/combat/` (M3 C2), since a zone's spawners name the kinds it spawns and a
//! scenario on it needn't, with the moves each names by ID and the `.luau` it names as its `script`, each
//! found by its sidecar, so a kind and its script reach a fight from documents alone. Every `.timeline.ron`
//! beside a set of moves' document goes in too (M3 Z5a), since a move names its cinematic by that file's
//! name. `replay::document` and `replay::cinematics` read them.

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

/// The ID an enemy document names in its first `field:` line, such as its `moves` or its `script`.
fn id_field(text: &str, field: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let code = line.split("//").next().unwrap_or_default().trim_start();
        let rest = code.strip_prefix(field)?.strip_prefix(':')?;
        Some(rest.split('"').nth(1)?.to_owned())
    })
}

/// The ID in a sidecar's text.
fn id_in_sidecar(text: &str) -> Option<String> {
    text.lines().find_map(|line| Some(line.trim_start().strip_prefix("id:")?.split('"').nth(1)?.to_owned()))
}

/// What a kind of enemy can need, under `content/combat/`: each document with a sidecar whose name ends with one
/// of `kinds` (`.luau`, `.moves.ron`, `.enemy.ron`), by the ID in it, as paths from `root`, sorted.
fn sidecars_by_id(root: &Path, directory: &Path, kinds: &[&str], found: &mut Vec<(String, String)>) {
    let Ok(entries) = fs::read_dir(directory) else { return };
    let mut paths: Vec<PathBuf> = entries.map(|entry| entry.expect("a directory entry").path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            sidecars_by_id(root, &path, kinds, found);
            continue;
        }
        let Some(document) = path.to_str().and_then(|path| path.strip_suffix(".meta")) else { continue };
        if !kinds.iter().any(|kind| document.ends_with(kind)) {
            continue;
        }
        let text = fs::read_to_string(&path).expect("a sidecar");
        let id = id_in_sidecar(&text).unwrap_or_else(|| panic!("{} has no ID", path.display()));
        let relative = Path::new(document).strip_prefix(root).expect("under the root").to_owned();
        found.push((id, relative.to_str().expect("a UTF-8 path").replace('\\', "/")));
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
    // Every kind of enemy, and the moves and script each names by ID: what they are is only found by
    // looking. A kind's name is its file's, so two of one name would be one name for two kinds.
    let content = root.join("content/combat");
    println!("cargo:rerun-if-changed={}", content.display());
    let mut found = Vec::new();
    sidecars_by_id(&root, &content, &[".luau", ".moves.ron", ".enemy.ron"], &mut found);
    let mut kinds = BTreeSet::new();
    for (_, kind) in found.iter().filter(|(_, relative)| relative.ends_with(".enemy.ron")) {
        let file = kind.rsplit('/').next().expect("a file");
        assert!(kinds.insert(file.to_owned()), "two kinds of enemy are {file}: a kind's name is its file's");
        let text = fs::read_to_string(root.join(kind)).expect("a kind of enemy");
        named.insert(kind.clone());
        for (field, what) in [("moves", ".moves.ron"), ("script", ".luau")] {
            let Some(id) = id_field(&text, field) else { continue };
            let (_, relative) = found
                .iter()
                .find(|(found, relative)| *found == id && relative.ends_with(what))
                .unwrap_or_else(|| {
                    panic!("{kind} names the {field} {id}, which no `{what}` under content/combat/ is")
                });
            named.insert(relative.clone());
        }
    }
    // The cinematics a set's moves can name (M3 Z5a): every timeline beside its document.
    let sets: Vec<String> =
        named.iter().filter(|relative| relative.ends_with(".moves.ron")).cloned().collect();
    for set in &sets {
        let folder = root.join(set).parent().expect("a set's document is in a folder").to_owned();
        println!("cargo:rerun-if-changed={}", folder.display());
        let mut timelines: Vec<String> = fs::read_dir(&folder)
            .expect("a set's folder")
            .map(|entry| entry.expect("a directory entry").file_name().to_string_lossy().into_owned())
            .filter(|file| file.ends_with(".timeline.ron"))
            .collect();
        timelines.sort();
        let prefix = set.rsplit_once('/').map_or(String::new(), |(folder, _)| format!("{folder}/"));
        named.extend(timelines.into_iter().map(|file| format!("{prefix}{file}")));
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
