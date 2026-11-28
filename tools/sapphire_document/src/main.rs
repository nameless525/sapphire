//! `sapphire_document merge <base> <ours> <theirs> [-o <out>] [--name <path>]`: git's merge driver for the
//! documents this crate knows (M6 T7). It merges the two sides by property path ([`sapphire_document::merge`]),
//! writes the result to `<out>` (or standard output), says on standard error how it merged and where they
//! clash, and exits as git expects of a driver: 0 merged, 1 with conflicts marked, 2 if it couldn't run. Once
//! it has the three files, it never leaves ours' bytes unmarked: a side that isn't UTF-8 text, or a merge that
//! fails, merges by lines instead, and the result is written beside `<out>` and renamed over it, so a failed
//! write leaves `<out>` as it was.
//! `--name` is the file's path in the tree (git's `%P`), which says its kind; without it, `<ours>` does.
//!
//! A clone turns it on with one `git config` line (docs/getting-started.md, "Merging documents"), naming the
//! driver `.gitattributes` gives the document kinds.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str =
    "usage: sapphire_document merge <base> <ours> <theirs> [-o <out>] [--name <path in the tree>]";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(message) => {
            eprintln!("sapphire_document: {message}");
            ExitCode::from(2)
        }
    }
}

/// Runs the command: whether it merged cleanly.
fn run(args: Vec<String>) -> Result<bool, String> {
    let mut args = args.into_iter();
    if args.next().as_deref() != Some("merge") {
        return Err(USAGE.to_owned());
    }
    let (mut files, mut out, mut name) = (Vec::new(), None, None);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" => out = Some(PathBuf::from(args.next().ok_or(USAGE)?)),
            "--name" => name = Some(PathBuf::from(args.next().ok_or(USAGE)?)),
            _ if arg.starts_with('-') && arg.len() > 1 => {
                return Err(format!("unknown argument {arg:?}\n{USAGE}"));
            }
            _ => files.push(PathBuf::from(arg)),
        }
    }
    let [base, ours, theirs] = <[PathBuf; 3]>::try_from(files).map_err(|_| USAGE.to_owned())?;
    let read = |path: &PathBuf| {
        std::fs::read(path).map_err(|error| format!("can't read {}: {error}", path.display()))
    };
    let sides = [read(&base)?, read(&ours)?, read(&theirs)?];
    let name = name.unwrap_or_else(|| ours.clone());
    let texts: Result<Vec<&str>, _> = sides.iter().map(|side| std::str::from_utf8(side)).collect();
    let Ok(&[base_text, ours_text, theirs_text]) = texts.as_deref() else {
        // Not text this crate can read: git's own merge, by lines of bytes, marked.
        let (text, conflicts) = sapphire_document::merge::merge_lines_bytes(&sides[0], &sides[1], &sides[2]);
        put(out.as_deref(), &text)?;
        eprintln!("sapphire_document merge: {}: merged by lines: a side isn't UTF-8 text", name.display());
        for line in &conflicts {
            eprintln!("  conflict at line {line}");
        }
        return Ok(conflicts.is_empty());
    };
    let merged = std::panic::catch_unwind(|| {
        sapphire_document::merge::merge(&name, base_text, ours_text, theirs_text)
    })
    .unwrap_or_else(|_| {
        // A fault in the merge by path is this tool's, never the documents': merge them by lines.
        let lines = sapphire_document::merge::merge_lines(base_text, ours_text, theirs_text);
        let at = |line: &usize| sapphire_document::merge::Conflict { at: format!("line {line}"), why: None };
        sapphire_document::merge::Merged {
            conflicts: lines.conflicts.iter().map(at).collect(),
            text: lines.text,
            how: sapphire_document::merge::How::ByLines { why: "the merge by path failed".to_owned() },
            notes: Vec::new(),
        }
    });
    put(out.as_deref(), merged.text.as_bytes())?;
    eprintln!("sapphire_document merge: {}: {}", name.display(), merged.summary());
    for conflict in &merged.conflicts {
        let at = if conflict.at.is_empty() { "the document" } else { &conflict.at };
        match &conflict.why {
            Some(why) => eprintln!("  conflict at {at}: {why}"),
            None => eprintln!("  conflict at {at}"),
        }
    }
    for note in &merged.notes {
        eprintln!("  note: {note}");
    }
    Ok(merged.is_clean())
}

/// Writes `text` to `out`, through a file beside it renamed over it, or to standard output.
fn put(out: Option<&Path>, text: &[u8]) -> Result<(), String> {
    let Some(out) = out else {
        return std::io::stdout().write_all(text).map_err(|error| format!("can't write the merge: {error}"));
    };
    let mut beside = out.as_os_str().to_owned();
    beside.push(".merging");
    let beside = PathBuf::from(beside);
    let written = std::fs::write(&beside, text).and_then(|()| std::fs::rename(&beside, out));
    written.map_err(|error| {
        let _ = std::fs::remove_file(&beside);
        format!("can't write {}: {error}", out.display())
    })
}
