//! The merge driver through a real `git merge` (M6 T7), in a scratch repository with the repository's own
//! `.gitattributes` and the one `git config` line a clone runs (docs/getting-started.md). Unix only: git runs a
//! driver's line through its shell, and how Git for Windows' shell takes a Windows path to the binary is unproven
//! here, with no Windows machine to run it on.
#![cfg(unix)]

mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

use sapphire_document::{Document, PropertyPath, kinds, registry_of};
use sapphire_moves::MoveSetDocument;

/// Where a document sits in the scratch repository: under `content/`, so `.gitattributes` names its driver.
const MOVES: &str = "content/combat/scratch.moves.ron";

/// A scratch repository, kept apart from the user's and the system's git settings: its own home, no global or
/// system config, and nothing of a git it runs under (a hook's `GIT_INDEX_FILE`, `GIT_CONFIG_PARAMETERS`).
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("an old scratch repository goes");
        }
        std::fs::create_dir_all(root.join("content/combat")).expect("a scratch folder");
        let scratch = Self { root };
        scratch.git(&["init", "-q", "-b", "main"]);
        for (key, value) in [
            ("user.name", "Merge Test"),
            ("user.email", "merge@example.invalid"),
            ("commit.gpgsign", "false"),
        ] {
            scratch.git(&["config", key, value]);
        }
        std::fs::write(scratch.root.join(".gitattributes"), support::read(".gitattributes"))
            .expect("attributes");
        std::fs::write(scratch.root.join(".git/info/exclude"), "/target/\n").expect("the build stays out");
        scratch
    }

    /// Turns the driver on with getting-started's line, word for word, over this build's binary where the line
    /// looks for it.
    fn configure_driver(&self) {
        self.configure_driver_built(true);
    }

    /// Getting-started's line, with this build's binary where it looks, or with none there (a worktree with no
    /// build).
    fn configure_driver_built(&self, built: bool) {
        let guide = support::read("docs/getting-started.md");
        let line = guide
            .lines()
            .find_map(|line| line.strip_prefix("git config merge.sapphire-document.driver "))
            .expect("getting-started gives the line");
        let driver = line.strip_prefix('\'').and_then(|line| line.strip_suffix('\'')).expect("quoted");
        let binary =
            driver.strip_prefix("b=").and_then(|line| line.split(';').next()).expect("the binary's path");
        if built {
            let at = self.root.join(binary);
            std::fs::create_dir_all(at.parent().expect("a folder")).expect("a build folder");
            std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_sapphire_document"), at).expect("the binary");
        }
        self.git(&["config", "merge.sapphire-document.driver", driver]);
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_in("", args)
    }

    fn run_in(&self, folder: &str, args: &[&str]) -> Output {
        let mut git = Command::new("git");
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                git.env_remove(key);
            }
        }
        git.args(args)
            .current_dir(self.root.join(folder))
            .env("HOME", &self.root)
            .env("XDG_CONFIG_HOME", &self.root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .expect("git runs")
    }

    fn git(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).expect("text")
    }

    fn write(&self, text: impl AsRef<[u8]>) {
        std::fs::write(self.root.join(MOVES), text).expect("a document");
    }

    fn read(&self) -> String {
        std::fs::read_to_string(self.root.join(MOVES)).expect("a document")
    }

    fn commit(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
    }
}

fn moves(text: &str) -> Document<MoveSetDocument> {
    Document::open(text.to_owned(), kinds::moves(), Arc::new(registry_of::<MoveSetDocument>()))
}

/// The player's moves with a set of `path` to `value` made.
fn edited(base: &str, edits: &[(&str, &str)]) -> String {
    let mut document = moves(base);
    for (path, value) in edits {
        let path: PropertyPath = path.parse().expect("a path");
        document.set_text("test", &path, value).unwrap_or_else(|why| panic!("{path}: {why}"));
    }
    document.text().to_owned()
}

/// A move of the player's, and two of its numbers.
fn a_move(base: &str) -> (String, String, String) {
    let document = moves(base);
    let first = &document.value().expect("reads").moves[0];
    (first.name.clone(), format!("{}", first.startup), format!("{}", first.recovery))
}

/// Two branches that edit different fields of one move, and one that adds a move, merge through git with no
/// conflict; two that set one field apart stop the merge, leave the file marked so it doesn't read, and git
/// lists it as unmerged.
#[test]
fn git_merges_documents_through_the_driver() {
    let base = support::read("content/combat/player/player.moves.ron");
    let (name, startup, recovery) = a_move(&base);
    let startup_path = format!(r#"moves["{name}"].startup"#);
    let recovery_path = format!(r#"moves["{name}"].recovery"#);
    let later = |number: &str| (number.parse::<u32>().expect("a whole number") + 3).to_string();

    let scratch = Scratch::new("merge-driver-clean");
    scratch.configure_driver();
    scratch.write(&base);
    scratch.commit("base");
    scratch.git(&["checkout", "-q", "-b", "theirs"]);
    let theirs = edited(&base, &[(&recovery_path, &later(&recovery))]);
    scratch.write(&theirs);
    scratch.commit("theirs: recovery");
    scratch.git(&["checkout", "-q", "main"]);
    let ours = edited(&base, &[(&startup_path, &later(&startup))]);
    scratch.write(&ours);
    scratch.commit("ours: startup");
    // From a folder inside the clone: git runs the driver from its root, where the line's path starts.
    let output = scratch.run_in("content", &["merge", "-q", "--no-edit", "theirs"]);
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{said}");
    assert!(said.contains("merged by property path"), "the driver ran and said how: {said}");
    let merged = scratch.read();
    let expected = edited(&ours, &[(&recovery_path, &later(&recovery))]);
    assert_eq!(moves(&merged).value(), moves(&expected).value());
    assert_eq!(scratch.git(&["status", "--porcelain"]), "", "merged and committed");

    let scratch = Scratch::new("merge-driver-conflict");
    scratch.configure_driver();
    scratch.write(&base);
    scratch.commit("base");
    scratch.git(&["checkout", "-q", "-b", "theirs"]);
    scratch.write(edited(&base, &[(&startup_path, &later(&startup))]));
    scratch.commit("theirs: startup");
    scratch.git(&["checkout", "-q", "main"]);
    scratch.write(edited(&base, &[(&startup_path, &later(&later(&startup)))]));
    scratch.commit("ours: startup");
    let output = scratch.run(&["merge", "--no-edit", "theirs"]);
    let said = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "a conflict stops the merge: {said}");
    assert!(said.contains(&format!("conflict at {startup_path}")), "{said}");
    let merged = scratch.read();
    assert!(merged.contains(&format!("<<<<<<< ours: {startup_path}")), "{merged}");
    assert!(MoveSetDocument::parse(&merged).is_err(), "a marked file doesn't read");
    assert_eq!(scratch.git(&["diff", "--name-only", "--diff-filter=U"]), format!("{MOVES}\n"));
}

/// Without the `git config` line, `.gitattributes` names a driver git doesn't have, and git merges by lines as it
/// always did: the driver is opt-in.
#[test]
fn without_the_config_line_git_merges_by_lines() {
    let base = support::read("content/combat/player/player.moves.ron");
    let (name, _, _) = a_move(&base);
    let startup_path = format!(r#"moves["{name}"].startup"#);
    let scratch = Scratch::new("merge-driver-off");
    scratch.write(&base);
    scratch.commit("base");
    scratch.git(&["checkout", "-q", "-b", "theirs"]);
    scratch.write(edited(&base, &[(&startup_path, "1")]));
    scratch.commit("theirs");
    scratch.git(&["checkout", "-q", "main"]);
    scratch.write(edited(&base, &[(&startup_path, "2")]));
    scratch.commit("ours");
    let output = scratch.run(&["merge", "--no-edit", "theirs"]);
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success() && !said.contains("property path"), "{said}");
    let merged = scratch.read();
    assert!(merged.contains("<<<<<<< HEAD"), "git's own markers: {merged}");
}

/// One field both sides set apart, merged through git in a scratch repository whose driver is `configured`: what
/// git said and the file it left.
fn clash(name: &str, base: &[u8], ours: &[u8], theirs: &[u8], built: bool) -> (Scratch, Output) {
    let scratch = Scratch::new(name);
    scratch.configure_driver_built(built);
    scratch.write(base);
    scratch.commit("base");
    scratch.git(&["checkout", "-q", "-b", "theirs"]);
    scratch.write(theirs);
    scratch.commit("theirs");
    scratch.git(&["checkout", "-q", "main"]);
    scratch.write(ours);
    scratch.commit("ours");
    let output = scratch.run(&["merge", "--no-edit", "theirs"]);
    (scratch, output)
}

/// A worktree with no build of the driver still gets a merge by lines with its conflicts marked, never ours'
/// bytes unmarked: the line falls back to `git merge-file`.
#[test]
fn with_no_build_the_line_merges_by_lines_marked() {
    let base = support::read("content/combat/player/player.moves.ron");
    let (name, _, _) = a_move(&base);
    let startup_path = format!(r#"moves["{name}"].startup"#);
    let (ours, theirs) = (edited(&base, &[(&startup_path, "2")]), edited(&base, &[(&startup_path, "1")]));
    let (scratch, output) =
        clash("merge-driver-unbuilt", base.as_bytes(), ours.as_bytes(), theirs.as_bytes(), false);
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success() && !said.contains("property path"), "{said}");
    let merged = scratch.read();
    for marker in ["<<<<<<< ours", "||||||| base", "=======", ">>>>>>> theirs"] {
        assert!(merged.contains(marker), "{marker}: {merged}");
    }
    assert_eq!(scratch.git(&["diff", "--name-only", "--diff-filter=U"]), format!("{MOVES}\n"));
}

/// A side the driver can't read as text (not UTF-8) merges by lines of bytes, marked, and exits 1: the driver's
/// own trouble never leaves ours' bytes unmarked.
#[test]
fn a_side_that_isnt_text_merges_by_lines_marked() {
    let base = support::read("content/combat/player/player.moves.ron");
    let bytes = |line: &[u8]| [base.as_bytes(), b"// \xff ", line, b"\n"].concat();
    let (base, ours, theirs) = (bytes(b"base"), bytes(b"ours"), bytes(b"theirs"));
    let (scratch, output) = clash("merge-driver-bytes", &base, &ours, &theirs, true);
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("a side isn't UTF-8 text"), "the driver ran and said why: {said}");
    assert!(!output.status.success(), "{said}");
    let merged = std::fs::read(scratch.root.join(MOVES)).expect("a document");
    let marked = [
        b"<<<<<<< ours\n// \xff ours\n||||||| base\n// \xff base\n=======\n" as &[u8],
        b"// \xff theirs\n>>>>>>> theirs\n",
    ]
    .concat();
    assert!(merged.ends_with(&marked), "{}", String::from_utf8_lossy(&merged));
    assert_eq!(scratch.git(&["diff", "--name-only", "--diff-filter=U"]), format!("{MOVES}\n"));
}
