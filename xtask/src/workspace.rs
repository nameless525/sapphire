//! Workspace paths and process helpers shared by every command.

use std::ffi::{OsStr, OsString};
use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, bail, ensure};
use serde::Deserialize;

/// The workspace root (the parent of `xtask/`).
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask lives inside the workspace").to_owned()
}

/// Where xtask writes cooked content and bundles: `<root>/target`, independent of
/// `CARGO_TARGET_DIR`, so the paths in docs and scripts stay stable.
pub fn output_dir() -> PathBuf {
    root().join("target")
}

static LOCKED: AtomicBool = AtomicBool::new(false);

/// Makes every cargo command xtask runs pass `--locked` (`cargo xtask --locked …`).
pub fn set_locked(locked: bool) {
    LOCKED.store(locked, Ordering::Relaxed);
}

/// `cargo <subcommand>` run from the workspace root with the cargo that runs xtask, plus
/// `--locked` when xtask was given it.
pub fn cargo(subcommand: &str) -> Command {
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command.current_dir(root()).arg(subcommand);
    if LOCKED.load(Ordering::Relaxed) {
        command.arg("--locked");
    }
    command
}

/// Runs `command`, streaming its output, and fails unless it exits successfully.
pub fn run(command: &mut Command) -> anyhow::Result<()> {
    eprintln!("$ {}", describe(command));
    let status = command.status().with_context(|| format!("failed to start `{}`", describe(command)))?;
    ensure!(status.success(), "`{}` failed ({status})", describe(command));
    Ok(())
}

/// Runs `command` and returns its stdout; stderr streams through.
pub fn output(command: &mut Command) -> anyhow::Result<String> {
    let output = command
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("failed to start `{}`", describe(command)))?;
    ensure!(output.status.success(), "`{}` failed ({})", describe(command), output.status);
    String::from_utf8(output.stdout)
        .with_context(|| format!("`{}` printed non-UTF-8 output", describe(command)))
}

/// A package to build with cargo.
pub struct CargoBuild<'a> {
    pub package: &'a str,
    /// Rust target triple; `None` builds for the host.
    pub target: Option<&'a str>,
    pub release: bool,
    /// Extra environment for cargo and everything it runs: build scripts, CMake, the linker.
    pub env: &'a [(&'a str, OsString)],
}

impl CargoBuild<'_> {
    /// `cargo <subcommand>` for this package, reporting artifacts as JSON for
    /// [`build_artifact`]. Append target-selection flags, then any `-- <rustc args>`.
    pub fn command(&self, subcommand: &str) -> Command {
        let mut command = cargo(subcommand);
        command.args(["--package", self.package, "--message-format=json-render-diagnostics"]);
        command.envs(self.env.iter().map(|(name, value)| (name, value)));
        if let Some(target) = self.target {
            command.args(["--target", target]);
        }
        if self.release {
            command.arg("--release");
        }
        command
    }
}

/// Builds binary `bin` and returns the path of its executable.
pub fn build_binary(build: &CargoBuild<'_>, bin: &str) -> anyhow::Result<PathBuf> {
    let mut command = build.command("build");
    command.args(["--bin", bin]);
    build_artifact(&mut command, bin, |artifact| {
        let is_bin = artifact.target.name == bin && artifact.target.kind.iter().any(|kind| kind == "bin");
        if is_bin { artifact.executable } else { None }
    })
}

/// Runs a cargo build from [`CargoBuild::command`] and returns the output file `pick`
/// selects from the artifacts cargo reports (the last one, if several match), wherever
/// `CARGO_TARGET_DIR` and the profile put it. `what` names the build in errors.
pub fn build_artifact(
    command: &mut Command,
    what: &str,
    mut pick: impl FnMut(Artifact) -> Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    eprintln!("$ {}", describe(command));
    let mut child = command
        .stdout(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start `{}`", describe(command)))?;

    // Diagnostics are rendered to stderr; stdout carries one JSON message per line.
    let stdout = child.stdout.take().expect("stdout is piped");
    let mut picked = None;
    for line in BufReader::new(stdout).lines() {
        let line = line.context("failed to read cargo output")?;
        if let Ok(CargoMessage::CompilerArtifact(artifact)) = serde_json::from_str(&line)
            && let Some(path) = pick(artifact)
        {
            picked = Some(path);
        }
    }
    let status = child.wait()?;
    ensure!(status.success(), "building `{what}` failed ({status})");
    match picked {
        Some(path) => Ok(path),
        None => bail!("cargo built `{what}` but reported no matching artifact"),
    }
}

#[derive(Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
enum CargoMessage {
    CompilerArtifact(Artifact),
    #[serde(other)]
    Other,
}

/// A `compiler-artifact` message: one compiled target and the files it produced.
#[derive(Deserialize)]
pub struct Artifact {
    pub target: ArtifactTarget,
    pub filenames: Vec<PathBuf>,
    pub executable: Option<PathBuf>,
}

#[derive(Deserialize)]
pub struct ArtifactTarget {
    pub name: String,
    pub kind: Vec<String>,
}

/// Recursively copies a directory tree.
pub fn copy_dir(from: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(to).with_context(|| format!("failed to create {}", to.display()))?;
    for entry in std::fs::read_dir(from).with_context(|| format!("failed to read {}", from.display()))? {
        let entry = entry?;
        let (source, dest) = (entry.path(), to.join(entry.file_name()));
        if entry.file_type()?.is_dir() {
            copy_dir(&source, &dest)?;
        } else {
            std::fs::copy(&source, &dest)
                .with_context(|| format!("failed to copy {} to {}", source.display(), dest.display()))?;
        }
    }
    Ok(())
}

/// Deletes a directory tree if it exists.
pub fn remove_dir_if_exists(dir: &Path) -> anyhow::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("failed to remove {}", dir.display()))
        }
        _ => Ok(()),
    }
}

/// `command` as a shell line, with the environment it sets, for `--dry-run`.
pub fn shell_line(command: &Command) -> String {
    let env = command.get_envs().filter_map(|(name, value)| {
        Some(format!("{}={}", name.to_string_lossy(), shell_word(&value?.to_string_lossy())))
    });
    let program = std::iter::once(command.get_program()).chain(command.get_args());
    env.chain(program.map(|arg| shell_word(&arg.to_string_lossy()))).collect::<Vec<_>>().join(" ")
}

/// Quotes `word` for a POSIX shell when it needs it.
fn shell_word(word: &str) -> String {
    let plain =
        !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c));
    if plain { word.to_owned() } else { format!("'{}'", word.replace('\'', r"'\''")) }
}

fn describe(command: &Command) -> String {
    let program = Path::new(command.get_program())
        .file_name()
        .map_or_else(|| command.get_program().to_string_lossy(), OsStr::to_string_lossy);
    std::iter::once(program)
        .chain(command.get_args().map(OsStr::to_string_lossy))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_trees_and_removes_them() {
        let scratch = std::env::temp_dir().join(format!("xtask-copy-{}", std::process::id()));
        let (from, to) = (scratch.join("from"), scratch.join("to"));
        std::fs::create_dir_all(from.join("a/b")).unwrap();
        std::fs::write(from.join("a/b/c.txt"), "c").unwrap();
        std::fs::write(from.join("top.txt"), "top").unwrap();

        copy_dir(&from, &to).unwrap();
        assert_eq!(std::fs::read_to_string(to.join("a/b/c.txt")).unwrap(), "c");
        assert_eq!(std::fs::read_to_string(to.join("top.txt")).unwrap(), "top");

        remove_dir_if_exists(&scratch).unwrap();
        assert!(!scratch.exists());
        remove_dir_if_exists(&scratch).unwrap();
    }

    #[test]
    fn reads_artifact_messages() {
        let artifact = r#"{"reason":"compiler-artifact","target":{"name":"app","kind":["lib"],
            "crate_types":["cdylib"]},"filenames":["/t/libapp.so"],"executable":null,"fresh":true}"#;
        let Ok(CargoMessage::CompilerArtifact(artifact)) = serde_json::from_str(artifact) else {
            panic!("not read as an artifact");
        };
        assert_eq!((artifact.target.name.as_str(), artifact.target.kind), ("app", vec!["lib".to_owned()]));
        assert_eq!(artifact.filenames, [PathBuf::from("/t/libapp.so")]);
        let finished = serde_json::from_str(r#"{"reason":"build-finished","success":true}"#);
        assert!(matches!(finished, Ok(CargoMessage::Other)));
    }

    #[test]
    fn prints_dry_run_shell_lines() {
        let mut command = Command::new("/bin/app");
        command.args(["--frames", "it's here"]).env("SAPPHIRE_CONTENT_DIR", "/t/content dir");
        assert_eq!(
            shell_line(&command),
            r"SAPPHIRE_CONTENT_DIR='/t/content dir' /bin/app --frames 'it'\''s here'"
        );
    }

    #[test]
    fn describes_commands_without_full_program_paths() {
        let mut command = Command::new("/usr/bin/xcrun");
        command.args(["simctl", "list"]);
        assert_eq!(describe(&command), "xcrun simctl list");
    }
}
