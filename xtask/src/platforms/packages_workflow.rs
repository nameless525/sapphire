//! Holds the nightly packages' workflow (M4 I2) to what packaging reads: a pull request runs
//! `.github/workflows/packages.yml` when it touches a file that `cargo xtask package` reads or that builds
//! the game for a package, and only then, since five shipping builds with fat LTO cost an hour of runners.
//!
//! Its `paths` filters must cover every repository file the package code names, every xtask module
//! `package` reaches from `platforms/` and the command line, and no other xtask module, so a change to
//! the goldens' runner alone builds no packages. Every filter must name something that exists.

use std::fs;
use std::path::Path;

use super::{ICON_FILE, IDENTITY_FILE, ios_store, privacy_manifest, steam, steampipe};
use crate::workspace;

const WORKFLOW: &str = ".github/workflows/packages.yml";

/// Repository files packaging reads, or that build or link the game only for a package: the package
/// code's own constants, the templates and projects it fills, and what compiles in the build number,
/// Windows' resources and the symbols' checker.
const READ: [&str; 18] = [
    WORKFLOW,
    IDENTITY_FILE,
    ICON_FILE,
    ios_store::ENTITLEMENTS_FILE,
    privacy_manifest::MANIFEST_FILE,
    steam::MACOS_ENTITLEMENTS,
    steam::WINDOWS_MANIFEST,
    steampipe::IDS_FILE,
    "platform/ios/Info.plist",
    "platform/macos/Info.plist",
    "platform/android/app/build.gradle.kts",
    ".cargo/config.toml",
    "rust-toolchain.toml",
    "game/game_app/build.rs",
    "game/game_app/Cargo.toml",
    "game/game_app/src/main.rs",
    "tools/sapphire_pipeline/src/icon.rs",
    "tools/sapphire_symbols/src/main.rs",
];

#[test]
fn the_packages_workflow_runs_on_what_packaging_reads() {
    let root = workspace::root();
    let text = fs::read_to_string(root.join(WORKFLOW)).unwrap();
    let filters = pull_request_paths(&text);
    assert!(!filters.is_empty(), "{WORKFLOW} has no pull_request paths");
    let runs = |path: &str| filters.iter().any(|filter| glob(filter, path));

    for path in READ {
        assert!(root.join(path).is_file(), "{path} is gone: update this list");
        assert!(runs(path), "{WORKFLOW} doesn't run on {path}, which packaging reads");
    }

    let src = root.join("xtask/src");
    let reached = reached_modules(&src);
    for entry in fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else { continue };
        if path.extension().is_none_or(|extension| extension != "rs") {
            continue;
        }
        let wanted = name == "main" || reached.iter().any(|module| module == name);
        let file = format!("xtask/src/{name}.rs");
        assert_eq!(
            runs(&file),
            wanted,
            "{file}: packaging {} it, so {WORKFLOW}'s filters should {}",
            if wanted { "reaches" } else { "never reaches" },
            if wanted { "name it" } else { "leave it out" }
        );
    }
    for entry in fs::read_dir(src.join("platforms")).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        assert!(runs(&format!("xtask/src/platforms/{name}")), "{WORKFLOW} doesn't run on platforms/{name}");
    }

    for filter in &filters {
        assert!(!filter.starts_with('!'), "teach this test negated filters: {filter}");
        let exists = match filter.split_once('*') {
            Some((dir, _)) => root.join(dir.trim_end_matches('/')).is_dir(),
            None => root.join(filter).is_file(),
        };
        assert!(exists, "{WORKFLOW} filters on {filter}, which names nothing");
    }
}

#[test]
fn filters_match_as_github_reads_them() {
    assert!(glob("platform/**", "platform/android/app/build.gradle.kts"));
    assert!(glob("xtask/src/platforms/**", "xtask/src/platforms/package.rs"));
    assert!(!glob("xtask/src/platforms/**", "xtask/src/golden.rs"));
    assert!(glob("xtask/src/*.rs", "xtask/src/main.rs"));
    assert!(!glob("xtask/src/*.rs", "xtask/src/platforms/mod.rs"));
    assert!(glob(".cargo/config.toml", ".cargo/config.toml"));
    assert!(!glob(".cargo/config.toml", ".cargo/config.toml.bak"));

    let text = "on:\n  pull_request:\n    paths:\n      - 'a/**'\n      - b.rs\n  schedule:\n    - cron: '0 0 * * *'\n";
    assert_eq!(pull_request_paths(text), ["a/**", "b.rs"]);
    assert_eq!(
        crate_modules("use crate::{content::{self, Cook}, release};\nuse crate::workspace;"),
        ["content", "release", "workspace"]
    );
}

/// The `on.pull_request.paths` list of a workflow, written as this repository writes its workflows.
fn pull_request_paths(workflow: &str) -> Vec<String> {
    let mut lines = workflow.lines().skip_while(|line| *line != "  pull_request:").skip(1);
    if lines.next() != Some("    paths:") {
        return Vec::new();
    }
    lines
        .map_while(|line| line.strip_prefix("      - "))
        .map(|item| item.trim().trim_matches('\'').trim_matches('"').to_owned())
        .collect()
}

/// Whether `path` matches `pattern` as GitHub's path filters read it: `**` matches anything, `*` anything
/// but a slash.
fn glob(pattern: &str, path: &str) -> bool {
    if let Some(rest) = pattern.strip_prefix("**") {
        return (0..=path.len()).any(|at| path.is_char_boundary(at) && glob(rest, &path[at..]));
    }
    match pattern.chars().next() {
        None => path.is_empty(),
        Some('*') => (0..=path.len())
            .filter(|&at| path.is_char_boundary(at))
            .take_while(|&at| !path[..at].contains('/'))
            .any(|at| glob(&pattern[1..], &path[at..])),
        Some(c) => path.starts_with(c) && glob(&pattern[c.len_utf8()..], &path[c.len_utf8()..]),
    }
}

/// The top-level xtask modules `platforms` reaches through `crate::` paths, itself included.
fn reached_modules(src: &Path) -> Vec<String> {
    let mut reached = vec!["platforms".to_owned()];
    let mut next = 0;
    while next < reached.len() {
        let module = reached[next].clone();
        next += 1;
        let mut texts = Vec::new();
        let file = src.join(format!("{module}.rs"));
        if file.is_file() {
            texts.push(fs::read_to_string(file).unwrap());
        }
        if src.join(&module).is_dir() {
            for entry in fs::read_dir(src.join(&module)).unwrap() {
                texts.push(fs::read_to_string(entry.unwrap().path()).unwrap());
            }
        }
        for text in texts {
            for used in crate_modules(&text) {
                let exists = src.join(format!("{used}.rs")).is_file() || src.join(&used).is_dir();
                if exists && !reached.contains(&used) {
                    reached.push(used);
                }
            }
        }
    }
    reached
}

/// The first segment of every `crate::` path in `text`, through `use crate::{a, b::c}` groups too.
fn crate_modules(text: &str) -> Vec<String> {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut found = Vec::new();
    for (at, _) in text.match_indices("crate::") {
        let rest = &text[at + "crate::".len()..];
        let Some(group) = rest.strip_prefix('{') else {
            found.push(rest.chars().take_while(|&c| ident(c)).collect());
            continue;
        };
        let (mut depth, mut item_start, mut name) = (1, true, String::new());
        for c in group.chars() {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                ',' if depth == 1 => item_start = true,
                c if depth == 1 && item_start && ident(c) => name.push(c),
                _ if !name.is_empty() => {
                    found.push(std::mem::take(&mut name));
                    item_start = false;
                }
                _ => {}
            }
            if depth == 0 {
                break;
            }
        }
        if !name.is_empty() {
            found.push(name);
        }
    }
    found.retain(|name: &String| !name.is_empty());
    found
}
