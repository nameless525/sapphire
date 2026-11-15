//! `cargo xtask bench-scale prepare|plan|run|report`: the benches at scale (M5 Z1).
//!
//! One batch measures every row exit tests 1, 2, 5, 6 and 7 ask for, on each tier, at any commit. The runs
//! are data, `bench/scale.ron`; `sapphire_testkit scale` plans them into sittings, runs them and writes the
//! report, since nothing may depend on a tool crate (rule R1). This command makes what the testkit runs
//! against, all under `target/bench-scale/<commit>/`:
//!
//! - `content/<preset>/<cook>/`: the shipped cook of `content/` with the preset's district beside it, one for
//!   each cook ([`bench::put_beside`]);
//! - `bin/`: the release `dev` sandbox (what B4 walked) and the testkit, copied so a later build can't change
//!   what a resumed batch runs;
//! - `replays/`: the crowd scenarios' replays, recorded here since none is committed, and the committed ones the
//!   stand-ins play;
//! - `logs/` (or `smoke/` for `--smoke`): each run's log, trace and status, and `report.md`.
//!
//! It never takes a gate slot itself: run `prepare` and `--smoke` through `q`, and timed sittings through
//! `slot --all --load 8` on a quiet machine.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

use crate::bench::{self, Preset};
use crate::cli::Args;
use crate::content::{self, Cook};
use crate::platforms;
use crate::replay;
use crate::workspace::{self, BuildProfile, CargoBuild};

/// The manifest of runs, relative to the workspace root.
pub const MANIFEST: &str = "bench/scale.ron";

const SANDBOX: &str = "sapphire_sandbox";
const TESTKIT: &str = "sapphire_testkit";

/// The crowd scenarios the manifest's runs replay: code scenarios with no committed replay
/// (`sapphire_gameplay::replay::chase::CROWD_SCENARIOS`), each recorded with the chase's laps.
const CROWDS: [&str; 2] = ["crowd-15", "crowd-60"];
const CROWD_SCRIPT: &str = "chase-laps";

/// Committed replays the stand-ins play, copied beside the crowds' so every run names `{replays}/…`
/// whatever folder the batch runs from.
const COMMITTED: [&str; 1] = ["zone-trip"];

/// The file `prepare` names its preset in, so `run` mounts what was prepared.
const PREPARED: &str = "preset";

pub fn run(mut args: Args) -> anyhow::Result<()> {
    let Some(command) = args.command() else {
        bail!("bench-scale expects `prepare`, `plan`, `run` or `report`");
    };
    let preset = args.value::<Preset>("--preset")?;
    let smoke = args.flag("--smoke");
    let folder = commit_dir()?;
    match command.as_str() {
        "prepare" => {
            args.finish(false)?;
            prepare(&folder, preset.map_or_else(manifest_preset, Ok)?)
        }
        "plan" => {
            args.finish(false)?;
            let mut testkit = testkit(&folder, "plan", smoke)?;
            workspace::run(&mut testkit)
        }
        "run" => {
            let sitting = args.value::<u32>("--sitting")?;
            let counts = args.flag("--counts");
            args.finish(false)?;
            let preset = match preset {
                Some(preset) => preset,
                None => prepared(&folder)?,
            };
            let content = folder.join("content").join(preset.name());
            let mut testkit = testkit(&folder, "run", smoke)?;
            testkit
                .arg("--sandbox")
                .arg(bin(&folder, SANDBOX))
                .arg("--desktop")
                .arg(content.join(Cook::Desktop.name()))
                .arg("--mobile")
                .arg(content.join(Cook::Mobile.name()))
                .arg("--replays")
                .arg(folder.join("replays"));
            if let Some(sitting) = sitting {
                testkit.arg("--sitting").arg(sitting.to_string());
            }
            if counts {
                testkit.arg("--counts");
            }
            workspace::run(&mut testkit)
        }
        "report" => {
            let baseline = args.value::<PathBuf>("--baseline")?;
            args.finish(false)?;
            let mut testkit = testkit(&folder, "report", smoke)?;
            let commit = folder.file_name().expect("keyed by commit").to_string_lossy().into_owned();
            testkit.arg("--commit").arg(commit);
            if let Some(baseline) = baseline {
                testkit.arg("--baseline").arg(baseline);
            }
            workspace::run(&mut testkit)
        }
        other => bail!("bench-scale expects `prepare`, `plan`, `run` or `report`, not `{other}`"),
    }
}

/// Cooks both cooks of `content/` and of the preset's district, puts each pair together, builds the sandbox
/// and the testkit into `bin/` and records the crowds' replays.
fn prepare(folder: &Path, preset: Preset) -> anyhow::Result<()> {
    fs::create_dir_all(folder)?;
    for cook in [Cook::Desktop, Cook::Mobile] {
        let shipped = content::cook(cook)?;
        let district = bench::cook(preset, cook)?;
        let out = folder.join("content").join(preset.name()).join(cook.name());
        bench::put_beside(&shipped.dir, &district.dir, &out, cook)?;
        eprintln!(
            "put the content and the {} district together for {cook} in {}",
            preset.name(),
            out.display()
        );
    }

    let env = platforms::host_build_env()?;
    let features = ["dev".to_owned()];
    let sandbox = workspace::build_binary(
        &CargoBuild {
            package: SANDBOX,
            target: None,
            profile: BuildProfile::Release,
            features: &features,
            env: &env,
        },
        SANDBOX,
    )?;
    let testkit = workspace::build_binary(
        &CargoBuild { package: TESTKIT, target: None, profile: BuildProfile::Dev, features: &[], env: &env },
        TESTKIT,
    )?;
    fs::create_dir_all(folder.join("bin"))?;
    for (built, name) in [(sandbox, SANDBOX), (testkit, TESTKIT)] {
        let to = bin(folder, name);
        fs::copy(&built, &to).with_context(|| format!("copying {} to {}", built.display(), to.display()))?;
    }

    let replays = folder.join("replays");
    fs::create_dir_all(&replays)?;
    for name in COMMITTED {
        let from = workspace::root().join("tests/replays").join(format!("{name}.sreplay"));
        fs::copy(&from, replays.join(format!("{name}.sreplay")))
            .with_context(|| format!("copying {}", from.display()))?;
    }
    let runner = replay::runner()?;
    for crowd in CROWDS {
        if replays.join(format!("{crowd}.sreplay")).is_file() {
            continue;
        }
        let mut record = Command::new(&runner);
        record
            .args(["record", crowd, "--dir"])
            .arg(&replays)
            .arg("--out")
            .arg(folder.join("recording"))
            .args(["--scenario", crowd, "--script", CROWD_SCRIPT]);
        workspace::run(&mut record)?;
    }
    fs::write(folder.join(PREPARED), preset.name())?;
    eprintln!("prepared the benches at scale ({} preset) in {}", preset.name(), folder.display());
    Ok(())
}

/// `sapphire_testkit scale <command>` from `bin/`, over the manifest and the logs folder.
fn testkit(folder: &Path, command: &str, smoke: bool) -> anyhow::Result<Command> {
    let testkit = bin(folder, TESTKIT);
    if !testkit.is_file() {
        bail!(
            "nothing is prepared for this commit in {}: run `cargo xtask bench-scale prepare`",
            folder.display()
        );
    }
    let mut process = Command::new(testkit);
    process
        .args(["scale", command, "--manifest"])
        .arg(workspace::root().join(MANIFEST))
        .arg("--out")
        .arg(folder.join(if smoke { "smoke" } else { "logs" }));
    if smoke {
        process.arg("--smoke");
    }
    Ok(process)
}

fn bin(folder: &Path, name: &str) -> PathBuf {
    folder.join("bin").join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

/// `target/bench-scale/<commit>/`: the short hash, with `-dirty` when the tree has changes, since a batch
/// measures exactly one tree.
fn commit_dir() -> anyhow::Result<PathBuf> {
    let root = workspace::root();
    let git = |args: &[&str]| workspace::output(Command::new("git").arg("-C").arg(&root).args(args));
    let hash = git(&["rev-parse", "--short=9", "HEAD"])?;
    let dirty = !git(&["status", "--porcelain"])?.trim().is_empty();
    let key = format!("{}{}", hash.trim(), if dirty { "-dirty" } else { "" });
    Ok(workspace::output_dir().join("bench-scale").join(key))
}

/// The preset the manifest names, read without the testkit's parser (xtask depends on no engine crate).
fn manifest_preset() -> anyhow::Result<Preset> {
    let path = workspace::root().join(MANIFEST);
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    preset_in(&text).with_context(|| format!("{} names no `preset: \"…\"`", path.display()))?.parse()
}

fn preset_in(text: &str) -> Option<&str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("//"))
        .find_map(|line| line.strip_prefix("preset:"))
        .and_then(|rest| rest.trim().strip_prefix('"'))
        .and_then(|rest| rest.split('"').next())
}

/// The preset `prepare` last made for this commit.
fn prepared(folder: &Path) -> anyhow::Result<Preset> {
    let path = folder.join(PREPARED);
    let name = fs::read_to_string(&path).with_context(|| {
        format!("nothing is prepared in {}: run `cargo xtask bench-scale prepare`", folder.display())
    })?;
    name.trim().parse()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifests_preset_is_read_from_its_text() {
        let text = "// preset: \"double\" is in a comment\n(\n    data: (\n        preset: \"budget\",\n";
        assert_eq!(preset_in(text), Some("budget"));
        assert_eq!(preset_in("(data: ())"), None);
        let committed = fs::read_to_string(workspace::root().join(MANIFEST)).expect("the committed manifest");
        assert!(preset_in(&committed).is_some_and(|name| name.parse::<Preset>().is_ok()));
    }
}
