//! `sapphire_pipeline content-check` (M6 T6): everything wrong with a source tree, by document and field, as the
//! editor's problems pane lists it, and what `cargo xtask content-check` and the opt-in pre-commit hook run.
//!
//! Three looks, each giving [`Problem`]s: every document of a kind the editor opens read as the editor reads it
//! (its parse failure, or each issue its rules find: [`Kind::read`]); the content rules ([`check::check`]), whose
//! errors no sidecar waives; and a cook of the whole tree into a scratch folder, whose refusal, if it refuses,
//! names its field as the editor's does ([`Refusal::of_cook`]). A fault two of them find is one problem
//! ([`problem::deduplicated`]). A cook stops at its first refusal, so a second refusal shows once the first is
//! mended; the documents and the rules are read whole.
//!
//! It fails on every problem it finds, whichever document has it: a document's problem can be another's doing (a
//! zone refused for a prefab a commit takes away), a cook stops at its first refusal, which may be any
//! document's, and the shipped tree reads clean, so a problem anywhere is one to mend before a commit. The files
//! it's given (the hook's, the ones a commit stages) must each be a file under the tree: one that isn't is a
//! typo, or a path it can't check, and is refused.

use std::fs;
use std::path::{Path, PathBuf};

use sapphire_document::Kind;
use sapphire_document::problem::{self, Problem};

use crate::check::{self, CheckError, Severity};
use crate::cook::{self, Cook, CookOptions};
use crate::refusal::{self, Refusal};

/// The exit code of a `content-check` that ran and found problems: any other failure exits 1 (or, through
/// cargo, as cargo does), so the hook and `cargo xtask content-check` tell the two apart.
pub const FOUND_PROBLEMS: i32 = 3;

/// What to check, and where its cook goes.
#[derive(Clone, Copy, Debug)]
pub struct Options<'a> {
    /// The source tree.
    pub source: &'a Path,
    /// The scratch folder the cook writes into: never a tree an app reads.
    pub out: &'a Path,
    pub cook: Cook,
    /// The cook's cache, if it keeps one.
    pub cache: Option<&'a Path>,
    /// Fail on a source without a sidecar, as CI cooks.
    pub locked: bool,
}

/// What a check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    /// Every problem of the tree.
    pub problems: Vec<Problem>,
    /// How many documents of a kind the editor opens it read.
    pub documents: usize,
}

/// A file a check was given that it can't check.
#[derive(Debug, thiserror::Error)]
pub enum ContentCheckError {
    #[error("{} isn't a file", .0.display())]
    Missing(PathBuf),
    #[error("{} isn't under the source tree {}", .file.display(), .source_tree.display())]
    Outside { file: PathBuf, source_tree: PathBuf },
}

impl Checked {
    /// Whether it passes: no problem anywhere in the tree.
    #[must_use]
    pub fn passes(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Checks the tree `options` names, once each of `touched` is a file under it.
pub fn content_check(options: &Options<'_>, touched: &[PathBuf]) -> Result<Checked, ContentCheckError> {
    let tree = same_file(options.source);
    for file in touched {
        if !file.is_file() {
            return Err(ContentCheckError::Missing(file.clone()));
        }
        if !same_file(file).starts_with(&tree) {
            return Err(ContentCheckError::Outside {
                file: file.clone(),
                source_tree: options.source.to_owned(),
            });
        }
    }
    let mut found = Vec::new();
    let documents = documents_of(options.source);
    for path in &documents {
        let Some(kind) = Kind::of(path) else { continue };
        match fs::read_to_string(path) {
            Ok(text) => found.extend(problem::of_text(path, kind, &text)),
            Err(error) => found.push(Problem::check(path, &format!("it doesn't read: {error}"))),
        }
    }
    match check::check(options.source) {
        Ok(report) => {
            for finding in report.findings.iter().filter(|finding| finding.fails()) {
                debug_assert_eq!(finding.severity, Severity::Error);
                let cook = finding.cook.map(|cook| format!(", {cook}")).unwrap_or_default();
                let path = options.source.join(&finding.source);
                // The finding's own message is read for its field, in its document's text; the rule follows it.
                let mut found_here = match fs::read_to_string(&path) {
                    Ok(text) => Problem::check_in(&path, &finding.message, &text),
                    Err(_) => Problem::check(&path, &finding.message),
                };
                found_here.message = format!("{} [{}{cook}]", finding.message, finding.rule);
                found.push(found_here);
            }
        }
        Err(error) => found.push(could_not_check(&error, options.source)),
    }
    let cooked = cook::cook_dir(
        options.source,
        options.out,
        &CookOptions { cook: options.cook, cache: options.cache, locked: options.locked, editor: false },
    );
    if let Err(error) = cooked {
        let refusal = Refusal::of_cook(&error);
        let document = refusal.source.clone().unwrap_or_else(|| options.source.to_owned());
        found.push(Problem::cook(&document, refusal.field.as_deref(), &refusal.message));
    }
    // Each tool names a document as it found it; the walk's name is the one printed, and one fault is one row.
    let walked: Vec<(PathBuf, &PathBuf)> = documents.iter().map(|path| (same_file(path), path)).collect();
    for problem in &mut found {
        let canonical = same_file(&problem.document);
        if let Some((_, path)) = walked.iter().find(|(file, _)| *file == canonical) {
            problem.document = (*path).clone();
        }
    }
    let problems = problem::deduplicated(found);
    let documents = documents.iter().filter(|path| Kind::of(path).is_some()).count();
    Ok(Checked { problems, documents })
}

/// A check that couldn't be made, on the source it stopped at.
fn could_not_check(error: &CheckError, root: &Path) -> Problem {
    let document = match error {
        CheckError::Cook { path, .. } | CheckError::Read { path, .. } | CheckError::Invalid { path, .. } => {
            path
        }
    };
    let document = if document.as_os_str().is_empty() { root } else { document };
    Problem::cook(document, None, &refusal::chain(error))
}

/// A path as one file is named whoever names it: canonical where it exists.
fn same_file(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

/// Every file under `root`, skipping hidden entries as a cook does, in a stable order.
fn documents_of(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut folders = vec![root.to_owned()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => folders.push(path),
                Ok(kind) if kind.is_file() => found.push(path),
                _ => {}
            }
        }
    }
    found.sort();
    found
}

/// What `content-check` prints: the problems as the pane lists them, then a line saying what it looked at.
#[must_use]
pub fn report(checked: &Checked) -> String {
    let mut text = problem::report(&checked.problems);
    text.push_str(&format!(
        "content-check: {} document(s) read, the rules and a cook run; {} problem(s)",
        checked.documents,
        checked.problems.len()
    ));
    text
}
