//! The opt-in pre-commit hook (M6 T6, `.githooks/pre-commit` and `.githooks/content-check`): in a scratch
//! repository, it lets a commit of a zone that cooks through, refuses one while any document of the tree has a
//! problem, printing it on its field, says when the check itself couldn't run, and does nothing unless it's turned
//! on.
//!
//! Unix only: the hook is a POSIX sh script, and whether Git for Windows' sh runs it with a `D:\…` pipeline as
//! its command is unproven here (no Windows machine to prove it on), so CI's Windows leg doesn't claim it.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const ZONE: &str = r#"(
    type: "sapphire::zone::ZoneDocument",
    version: 1,
    data: (
        grid: (cell_size: 32.0),
        lights: [(position: (0.0, 4.0, 0.0), color: (1.0, 0.9, 0.8), intensity: 2.0, range: 8.0)],
    ),
)
"#;

/// A repository of its own under the test's temporary folder, with the checkout's hooks in `.git/hooks` and the
/// privacy guard the pre-commit runs first.
fn scratch_repository(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("sapphire_problems_hook_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("content/zones")).unwrap();
    fs::create_dir_all(root.join(".github/scripts")).unwrap();
    git(&root, &["init", "-q"]);
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // Copies keep the hooks' modes.
    for hook in ["pre-commit", "content-check"] {
        fs::copy(checkout.join(".githooks").join(hook), root.join(".git/hooks").join(hook)).unwrap();
    }
    let guard = ".github/scripts/privacy_guard.py";
    fs::copy(checkout.join(guard), root.join(guard)).unwrap();
    root
}

/// Git in `root`, its hooks the repository's own whatever the user's global `core.hooksPath` says.
fn git_command(root: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(root).args([
        "-c",
        "core.hooksPath=.git/hooks",
        "-c",
        "user.name=test",
        "-c",
        "user.email=test@users.noreply.github.com",
        "-c",
        "commit.gpgsign=false",
    ]);
    command
}

fn git(root: &Path, args: &[&str]) -> Output {
    git_command(root).args(args).output().expect("git runs")
}

/// Commits what's staged with the hook running `check` (the pipeline built for these tests, by default).
fn commit_with(root: &Path, message: &str, check: &str) -> Output {
    git_command(root)
        .env("SAPPHIRE_CONTENT_CHECK", check)
        .args(["commit", "-q", "-m", message])
        .output()
        .expect("git runs")
}

fn pipeline_check(root: &Path) -> String {
    format!(
        "{} content-check --source content --out {}",
        env!("CARGO_BIN_EXE_sapphire_pipeline"),
        root.join("scratch").display()
    )
}

/// Stages `content/` and commits it.
fn commit(root: &Path, message: &str) -> Output {
    git(root, &["add", "-A", "content"]);
    commit_with(root, message, &pipeline_check(root))
}

fn said(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

fn commits(root: &Path) -> usize {
    String::from_utf8_lossy(&git(root, &["log", "--oneline"]).stdout).lines().count()
}

#[test]
fn the_hook_refuses_a_commit_while_the_content_has_a_problem() {
    let root = scratch_repository("refuses");
    let zone = root.join("content/zones/yard.zone.ron");
    let broken = ZONE.replace("intensity: 2.0", "intensity: -2.0");

    // Off until it's turned on: the pre-commit runs the privacy guard alone.
    fs::write(&zone, &broken).unwrap();
    let unchecked = commit(&root, "a broken zone, unchecked");
    assert!(unchecked.status.success(), "{}", said(&unchecked));
    git(&root, &["config", "sapphire.contentCheck", "true"]);

    fs::write(&zone, ZONE).unwrap();
    // The tree is checked whole, a missing sidecar included: an unlocked cook writes the zone's, which the
    // commit then holds, as a commit of a new document does.
    Command::new(env!("CARGO_BIN_EXE_sapphire_pipeline"))
        .current_dir(&root)
        .args(["content-check", "--source", "content", "--out"])
        .arg(root.join("scratch"))
        .output()
        .expect("the pipeline runs");
    let good = commit(&root, "a zone that cooks");
    assert!(good.status.success(), "{}", said(&good));

    fs::write(&zone, &broken).unwrap();
    let refused = commit(&root, "a zone whose light is negative");
    let out = said(&refused);
    assert!(!refused.status.success(), "the hook let it through: {out}");
    let document = Path::new("content").join("zones").join("yard.zone.ron");
    assert!(out.contains(&format!("{}\n  lights[0].intensity: ", document.display())), "{out}");
    let rows = out.lines().filter(|line| line.starts_with("  lights[0].intensity: ")).count();
    assert_eq!(rows, 1, "the parse and the cook find it once: {out}");
    assert!(out.contains("the content has problems"), "{out}");
    assert_eq!(commits(&root), 2, "the refused commit didn't land");

    // A problem in a document the commit doesn't stage fails it too: the tree is checked whole.
    fs::write(&zone, ZONE.replace("range: 8.0", "range: 9.0")).unwrap();
    let other = root.join("content/zones/other.zone.ron");
    fs::write(&other, &broken).unwrap();
    git(&root, &["add", "content/zones/yard.zone.ron"]);
    let elsewhere = commit_with(&root, "a good change beside a broken document", &pipeline_check(&root));
    assert!(!elsewhere.status.success(), "{}", said(&elsewhere));
    let other_document = Path::new("content").join("zones").join("other.zone.ron");
    assert!(said(&elsewhere).contains(&other_document.display().to_string()), "{}", said(&elsewhere));
    fs::remove_file(&other).unwrap();

    // A check that can't run says so, and refuses.
    let failed = commit_with(&root, "a check that fails", "false");
    assert!(!failed.status.success());
    assert!(said(&failed).contains("the check itself failed"), "{}", said(&failed));

    // A commit that touches nothing under content/ isn't checked.
    git(&root, &["reset", "-q", "content"]);
    fs::write(root.join("README"), "a note").unwrap();
    git(&root, &["add", "README"]);
    let note = commit_with(&root, "a note", "false");
    assert!(note.status.success(), "{}", said(&note));
    assert_eq!(commits(&root), 3);
    fs::remove_dir_all(root).unwrap();
}

/// A file the check is given must be a file under its tree: a typo, or a path git quoted, isn't let through.
#[test]
fn a_named_file_must_be_a_file_under_the_tree() {
    let root = scratch_repository("named");
    fs::write(root.join("content/zones/yard.zone.ron"), ZONE).unwrap();
    fs::write(root.join("README"), "a note").unwrap();
    for (file, says) in
        [("content/zones/yrad.zone.ron", "isn't a file"), ("README", "isn't under the source tree")]
    {
        let run = Command::new(env!("CARGO_BIN_EXE_sapphire_pipeline"))
            .current_dir(&root)
            .args(["content-check", "--source", "content", "--out"])
            .arg(root.join("scratch"))
            .arg(file)
            .output()
            .expect("the pipeline runs");
        assert_eq!(run.status.code(), Some(1), "refused, not a problem found: {}", said(&run));
        assert!(said(&run).contains(says), "{}", said(&run));
    }
    fs::remove_dir_all(root).unwrap();
}
