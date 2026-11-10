//! Compiles in the content's dialogues by name (M3 Z5e): `--ui-dialogue [<name>:]<lines>` previews
//! `content/dialogue/<name>.dialogue.ron` by the ID in its sidecar, so a new dialogue is a document and a
//! name, with no Rust. The cooked dialogue is what the preview walks: this holds only the name and the ID.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::{env, fs};

/// The ID in a sidecar's text.
fn id_in_sidecar(text: &str) -> Option<String> {
    text.lines().find_map(|line| Some(line.trim_start().strip_prefix("id:")?.split('"').nth(1)?.to_owned()))
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    // Joined, never formatted: on Windows a canonical root is a `\\?\` path, where `/` isn't a separator.
    let directory = manifest.join("..").join("..").join("content").join("dialogue");
    println!("cargo:rerun-if-changed={}", directory.display());
    println!("cargo:rerun-if-changed=build.rs");

    let mut sidecars: Vec<PathBuf> = fs::read_dir(&directory)
        .expect("content/dialogue")
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.to_string_lossy().ends_with(".dialogue.ron.meta"))
        .collect();
    sidecars.sort();

    let mut code = String::from(
        "/// Each dialogue in `content/dialogue/`, by its file's name before `.dialogue.ron`, and its ID.\n",
    );
    code.push_str("pub const DIALOGUES: &[(&str, &str)] = &[\n");
    for path in &sidecars {
        println!("cargo:rerun-if-changed={}", path.display());
        let file = path.file_name().and_then(|name| name.to_str()).expect("a UTF-8 name");
        let name = file.strip_suffix(".dialogue.ron.meta").expect("filtered");
        let text = fs::read_to_string(path).expect("a sidecar");
        let id = id_in_sidecar(&text).unwrap_or_else(|| panic!("{} has no ID", path.display()));
        writeln!(code, "    ({name:?}, {id:?}),").expect("a string");
    }
    code.push_str("];\n");

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets it"));
    fs::write(out.join("dialogues.rs"), code).expect("OUT_DIR is writable");
}
