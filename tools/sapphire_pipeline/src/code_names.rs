//! Finding the assets the engine's code names (`--code`, M3 W2).
//!
//! A pack's reach sees what documents name, and not a constant in the stage. So an asset named only by
//! code, like the look-dev scene's plants, is shared for as long as no zone reaches it, and the day one
//! zone does it moves to that zone's pack: every app that doesn't mount that zone loses it, and only on
//! the device that first tries. [`pack::check_code_names`](crate::pack::check_code_names) makes that a
//! failed cook, naming the line.
//!
//! A scan rather than a list, so there's nothing to keep up: any ID-shaped string literal in `.rs` files
//! under the root counts, however it's reached (`AssetId::from_text`, a `match`). What it can't see is an
//! ID computed at run time, such as a sub-asset's. M3 S3 turns these constants into lists documents
//! name, and what's left of this goes with the last of them.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sapphire_assets::AssetId;

/// An asset some code names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeName {
    pub id: AssetId,
    pub path: PathBuf,
    /// From 1.
    pub line: usize,
}

/// What's under `root` that isn't runtime code: tools link no game and name nothing an app needs, an
/// integration test runs where every pack is mounted, and a dot directory may hold whole other checkouts.
const SKIPPED: [&str; 4] = ["target", "tools", "xtask", "tests"];

/// Every asset named in the `.rs` files under `root`, in no particular order.
pub fn scan(root: &Path) -> io::Result<Vec<CodeName>> {
    let mut names = Vec::new();
    let mut directories = vec![root.to_owned()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if entry.file_type()?.is_dir() {
                if !name.starts_with('.') && !SKIPPED.contains(&&*name) {
                    directories.push(path);
                }
            } else if name.ends_with(".rs") {
                let text = fs::read_to_string(&path)?;
                names.extend(in_text(&text).map(|(line, id)| CodeName { id, path: path.clone(), line }));
            }
        }
    }
    Ok(names)
}

/// The ID-shaped string literals in `text`, with their lines, from 1, leaving out whole-line comments: a
/// doc's example names nothing an app needs.
fn in_text(text: &str) -> impl Iterator<Item = (usize, AssetId)> + '_ {
    let code = text.lines().enumerate().filter(|(_, line)| !line.trim_start().starts_with("//"));
    code.flat_map(|(index, line)| {
        line.split('"').filter_map(move |piece| piece.parse::<AssetId>().ok().map(|id| (index + 1, id)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: &str = "1ff3290e-5fa9-4f61-b891-2ca6d76b285f";
    const TWO: &str = "c6a8f3d2-5b47-4e19-8d0c-27f1b9e46a13";

    #[test]
    fn any_literal_of_an_ids_shape_counts_however_its_used() {
        let text = format!(
            "const A: AssetId = AssetId::from_text(\"{ONE}\");\n\nlet b = AssetId::from_text(match plant {{\n    Plant::Pine => \"{TWO}\",\n}});\nlet not = \"1ff3290e\";\n// \"not an id\"\n//! materials: {{\"timber\": \"{ONE}\"}},\n"
        );
        let found: Vec<(usize, String)> = in_text(&text).map(|(line, id)| (line, id.to_string())).collect();
        assert_eq!(found, [(1, ONE.to_owned()), (4, TWO.to_owned())]);
    }

    #[test]
    fn it_reads_runtime_code_and_leaves_out_tools_and_other_checkouts() {
        let root = std::env::temp_dir().join(format!("sapphire_code_names_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let write = |relative: &str, id: &str| {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, format!("\n\"{id}\"\n")).unwrap();
        };
        write("crates/stage/src/deep/lookdev.rs", ONE);
        write("tools/cooker/src/main.rs", TWO);
        write("xtask/src/main.rs", TWO);
        write("crates/stage/tests/walk.rs", TWO);
        write(".claude/worktrees/other/crates/stage/src/lib.rs", TWO);
        write("target/debug/build/out.rs", TWO);
        write("crates/stage/src/lookdev.txt", TWO);

        let names = scan(&root).unwrap();
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(names.len(), 1, "{names:?}");
        assert_eq!(names[0].id, ONE.parse().unwrap());
        assert_eq!(names[0].line, 2);
        assert!(names[0].path.ends_with("crates/stage/src/deep/lookdev.rs"));
    }
}
