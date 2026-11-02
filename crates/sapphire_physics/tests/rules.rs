//! The crate's rules, held in place by reading its own files.

use std::fs;
use std::path::Path;

fn crate_file(path: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).expect(path)
}

/// No engine type leaks `parry3d`: a Rapier, parry or Jolt upgrade touches only this crate.
#[test]
fn no_public_item_names_parry() {
    for entry in fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("src")).expect("src") {
        let path = entry.expect("an entry").path();
        let text = fs::read_to_string(&path).expect("a source file");
        for (number, line) in text.lines().enumerate() {
            let line = line.trim_start();
            if line.starts_with("pub ") {
                assert!(
                    !line.to_ascii_lowercase().contains("parry"),
                    "{}:{}: a public item names parry: {line}",
                    path.display(),
                    number + 1
                );
            }
        }
    }
}

/// `enhanced-determinism` would change glam for the whole process (M2 D3). Besides this reading of
/// the manifest, `lib.rs` asserts glam's layouts at compile time, so a build where any crate turned
/// the feature on doesn't compile.
#[test]
fn the_manifest_never_asks_for_enhanced_determinism() {
    for path in ["../../Cargo.toml", "Cargo.toml", "../sapphire_colliders/Cargo.toml"] {
        for line in crate_file(path).lines().filter(|line| !line.trim_start().starts_with('#')) {
            assert!(
                !line.contains("enhanced-determinism") && !line.contains("rapier"),
                "{path} asks for it again: {line}"
            );
        }
    }
}
