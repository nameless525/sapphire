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
//! name. `replay::document` and `replay::cinematics` read them. Every `content/inputs/*.inputs.ron` goes in too
//! (M4 R2), the scripted input a recording plays, which `replay::inputs` reads.
//!
//! The zones scenarios name in `shelf` go in as `BAKED_ZONES` (M4 R3), each with the copies of its cook in
//! `baked/zones/` (its `.szone`, `.scol` and `.snav`), since its navmesh takes a bake, which takes the
//! pipeline; and so does every `.encounter.ron` beside its document, since a shelved zone cooks the
//! encounters it lists. A shelved zone with no copies yet is left out with a warning, so the pipeline's
//! `zone_baked` test, which takes them, still builds. `replay::scenario` reads `BAKED_ZONES`.

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

/// The strings in a RON document's first `field: [...]` list, such as a scenario's `shelf`, which may run over
/// several lines. Comments are skipped.
fn list_field(text: &str, field: &str) -> Vec<String> {
    let mut code = text.lines().map(|line| line.split("//").next().unwrap_or_default());
    let Some(first) = code.by_ref().find_map(|line| line.trim_start().strip_prefix(field)?.strip_prefix(':'))
    else {
        return Vec::new();
    };
    let mut strings = Vec::new();
    for line in std::iter::once(first).chain(code) {
        let list = line.split(']').next().unwrap_or_default();
        strings.extend(list.split('"').skip(1).step_by(2).map(str::to_owned));
        if line.contains(']') {
            break;
        }
    }
    strings
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
    let mut shelved = BTreeSet::new();
    let mut code = String::from("pub(super) const SCENARIO_DOCUMENTS: &[(&str, &str)] = &[\n");
    for path in &scenarios {
        let text = fs::read_to_string(path).expect("a scenario document");
        named.extend(named_documents(&text));
        shelved.extend(list_field(&text, "shelf"));
        let file = path.file_name().and_then(|name| name.to_str()).expect("a UTF-8 name");
        let stem = file.strip_suffix(".scenario.ron").expect("filtered");
        writeln!(code, "    ({stem:?}, include_str!({:?})),", path.display().to_string()).expect("a string");
        println!("cargo:rerun-if-changed={}", path.display());
    }
    // Every scripted input (M4 R2), by name, as the scenarios are.
    let inputs = root.join("content/inputs");
    println!("cargo:rerun-if-changed={}", inputs.display());
    let mut documents: Vec<PathBuf> = fs::read_dir(&inputs)
        .expect("content/inputs")
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.to_string_lossy().ends_with(".inputs.ron"))
        .collect();
    documents.sort();
    code.push_str("];\n\npub(super) const INPUTS_DOCUMENTS: &[(&str, &str)] = &[\n");
    for path in &documents {
        let file = path.file_name().and_then(|name| name.to_str()).expect("a UTF-8 name");
        let stem = file.strip_suffix(".inputs.ron").expect("filtered");
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
    // The encounters a shelved zone lists, which its load cooks from their documents: every one beside it.
    for zone in &shelved {
        let folder = root.join(zone).parent().expect("a zone's document is in a folder").to_owned();
        println!("cargo:rerun-if-changed={}", folder.display());
        let prefix = zone.rsplit_once('/').map_or(String::new(), |(folder, _)| format!("{folder}/"));
        let mut encounters: Vec<String> = fs::read_dir(&folder)
            .unwrap_or_else(|_| panic!("a scenario shelves {zone}, which isn't there"))
            .map(|entry| entry.expect("a directory entry").file_name().to_string_lossy().into_owned())
            .filter(|file| file.ends_with(".encounter.ron"))
            .map(|file| format!("{prefix}{file}"))
            .collect();
        encounters.sort();
        named.extend(encounters);
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
    fs::write(out.join("baked_zones.rs"), baked_zones(&root, &manifest, &shelved))
        .expect("OUT_DIR is writable");
}

/// `BAKED_ZONES`, by stem, and `SHELVED_ZONES`: the stem of every zone a scenario shelves, copied or not.
fn baked_zones(root: &Path, manifest: &Path, shelved: &BTreeSet<String>) -> String {
    let copies = manifest.join("baked/zones");
    println!("cargo:rerun-if-changed={}", copies.display());
    let mut stems = BTreeSet::new();
    for zone in shelved {
        let file = zone.rsplit('/').next().expect("a file");
        let stem =
            file.strip_suffix(".zone.ron").unwrap_or_else(|| panic!("a scenario shelves {zone}, not a zone"));
        let sidecar = fs::read_to_string(root.join(format!("{zone}.meta")))
            .unwrap_or_else(|_| panic!("a scenario shelves {zone}, which has no sidecar"));
        let id = id_in_sidecar(&sidecar).unwrap_or_else(|| panic!("{zone}.meta has no ID"));
        let fresh = stems.iter().all(|(other, _): &(String, String)| other != stem);
        assert!(
            fresh,
            "scenarios shelve two zones called {stem}: a shelved zone is found by its file's name"
        );
        stems.insert((stem.to_owned(), id));
    }
    let mut code = String::from("pub const BAKED_ZONES: &[BakedZone] = &[\n");
    for (stem, id) in &stems {
        let copy = |extension: &str| copies.join(format!("{stem}.{extension}"));
        let paths = [copy("szone"), copy("scol"), copy("snav")];
        if !paths.iter().all(|path| path.is_file()) {
            println!(
                "cargo:warning=a scenario shelves {stem}, and baked/zones/ has no copy of its cook: `cargo xtask \
                 content`, then `SAPPHIRE_BAKE_ZONES=1 cargo test -p sapphire_pipeline --test zone_baked`"
            );
            continue;
        }
        let [zone, colliders, navmesh] = paths.map(|path| {
            println!("cargo:rerun-if-changed={}", path.display());
            format!("include_bytes!({:?})", path.display().to_string())
        });
        writeln!(
            code,
            "    BakedZone {{ asset: AssetId::from_text({id:?}), stem: {stem:?}, zone: {zone}, colliders: \
             {colliders}, navmesh: {navmesh} }},"
        )
        .expect("a string");
    }
    code.push_str("];\n\npub const SHELVED_ZONES: &[&str] = &[\n");
    for (stem, _) in &stems {
        writeln!(code, "    {stem:?},").expect("a string");
    }
    code.push_str("];\n");
    code
}
