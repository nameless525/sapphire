//! `cargo xtask bench-scale prepare|plan|run|report`: the benches at scale (M5 Z1).
//!
//! One batch measures every row exit tests 1, 2, 5, 6 and 7 ask for, and the packages' costs on and off beside
//! exit test 3, on each tier, at any commit, and its report gives a verdict for each exit test and tier. The runs
//! are data, `bench/scale.ron`; `sapphire_testkit scale` plans them into sittings, runs them and writes the
//! report, since nothing may depend on a tool crate (rule R1). This command makes what the testkit runs
//! against, all under `target/bench-scale/<commit>/`:
//!
//! - `content/<preset>/<cook>/`: the content at the preset with its district beside it, one for each cook
//!   ([`bench::put_beside`]): the shipped cook of `content/` for `ci`, and past it the shipped content with the
//!   party's kinds and the wave's at the preset's rows ([`bench::cook_kinds`], M5 C12b);
//! - `content/<preset>/<cook>-no-cell-levels/`: the same with the district's cells cooked with one level, what
//!   D6's pair measures its levels against (M5 Z2), a run's variant naming it (`content: Some("no-cell-levels")`);
//! - `bin/`: the release `dev` sandbox (what B4 walked) and the testkit, copied so a later build can't change
//!   what a resumed batch runs;
//! - `replays/`: the crowd scenarios' replays, recorded here since none is committed, and the committed ones the
//!   runs play;
//! - `logs/` (or `smoke/` for `--smoke`): each run's log, trace and status, and `report.md`.
//!
//! `device --platform ios-sim|android` (M5 Z2) walks the manifest's runs that name a device there: it puts the
//! mobile cook and the device's preset's district together in `content/<preset>/mobile/`, bundles the release
//! `dev` sandbox with them, boots a device of its own (a private simulator, or the headless emulator), installs
//! the bundle and has the testkit walk each unit through `launch` into `logs/`, beside this Mac's, then shuts the
//! device down. The device's verdicts are by counts alone.
//!
//! It never takes a gate slot itself: run `prepare` and `--smoke` through `q`, timed sittings through
//! `slot --all --load 8` on a quiet machine, and `device` inside the device slot.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail, ensure};

use crate::bench::{self, CellLevels, Preset};
use crate::cli::Args;
use crate::content::{self, Cook};
use crate::platforms::{self, App, Platform};
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

/// Committed replays the runs play, copied beside the crowds' so every run names `{replays}/…` whatever
/// folder the batch runs from: a stand-in's trip, the district's fight, the party of three kinds against
/// twelve (M5 C12b), the heroine's conversation close-up (M6 B8), the district's fight with her ultimate,
/// the fight that ships (M6 B5), and the nature zone's fight in its clearing (M6 W1).
const COMMITTED: [&str; 6] = [
    "zone-trip",
    "district-encounter",
    "party-three-kinds",
    "dialogue-closeup",
    "district-ultimate",
    "nature-encounter",
];

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
        "device" => {
            ensure!(preset.is_none(), "a device's preset is the manifest's, in its `devices`");
            let platform = args.required::<Platform>("--platform")?;
            let mut tiers = Vec::new();
            while let Some(tier) = args.value::<String>("--tier")? {
                tiers.push(tier);
            }
            let dry_run = args.flag("--dry-run");
            args.finish(false)?;
            device(&folder, platform, &tiers, smoke, dry_run)
        }
        // The testkit's launcher for a device's units: `device` passes it, with the device it booted.
        "launch" => {
            let platform = args.required::<Platform>("--platform")?;
            let device = args.required::<String>("--device")?;
            let app_args = args.finish(true)?;
            platforms::launch_on(platform, &device, &App::load(platforms::SANDBOX)?, &app_args)
        }
        other => bail!("bench-scale expects `prepare`, `plan`, `run`, `report` or `device`, not `{other}`"),
    }
}

/// Walks the manifest's runs for a device on it (M5 Z2): puts the mobile cook and the device's preset's district
/// together, bundles the release `dev` sandbox for `platform` with them, boots a device of its own headless
/// (a private simulator, or the emulator) and installs it, then has the testkit run each of the device's units
/// through `bench-scale launch` into the batch's `logs/` and print the device's verdicts, and shuts the device
/// down, however that went. With `dry_run`, it stops before booting.
fn device(
    folder: &Path,
    platform: Platform,
    tiers: &[String],
    smoke: bool,
    dry_run: bool,
) -> anyhow::Result<()> {
    ensure!(
        matches!(platform, Platform::IosSim | Platform::Android),
        "the benches at scale walk on `--platform ios-sim` or `android`, not `{platform}`"
    );
    let testkit = build_testkit(folder)?;
    let mut ask = Command::new(&testkit);
    ask.args(["scale", "preset", "--manifest"]).arg(workspace::root().join(MANIFEST));
    ask.args(["--device", platform.name()]);
    let preset: Preset = workspace::output(&mut ask)?.trim().parse()?;
    put_together(folder, preset, Cook::Mobile, &[CellLevels::On])?;

    let app = App::load(platforms::SANDBOX)?;
    let options = platforms::Options {
        release: true,
        features: vec!["dev".to_owned()],
        content: Some(content_folder(folder, preset, Cook::Mobile, CellLevels::On)),
        ..platforms::Options::default()
    };
    let bundle = platforms::bundle(platform, &app, &options)?;
    if dry_run {
        eprintln!(
            "dry run: bundled {} with the {} district; no device booted",
            bundle.path.display(),
            preset.name()
        );
        return Ok(());
    }
    let device = platforms::boot(platform, &app, &bundle)?;
    drop(bundle); // The device has its own copy now.
    let launcher = std::env::current_exe().context("finding xtask's own executable, the units' launcher")?;
    let mut walks = self::testkit(folder, "run", smoke)?;
    walks.args(["--device", platform.name()]);
    for tier in tiers {
        walks.args(["--tier", tier]);
    }
    walks.arg("--launch").arg(launcher).args(["bench-scale", "launch", "--platform", platform.name()]);
    walks.args(["--device", device.id()]);
    let walked = workspace::run(&mut walks);
    drop(device);
    walked
}

/// Cooks both cooks of the content at the preset (with its kinds past `ci`) and of the preset's district, with
/// its cells' levels and without (D6's pair, M5 Z2), puts each content and district together, builds the
/// sandbox and the testkit into `bin/` and records the crowds' replays.
fn prepare(folder: &Path, preset: Preset) -> anyhow::Result<()> {
    fs::create_dir_all(folder)?;
    for cook in [Cook::Desktop, Cook::Mobile] {
        put_together(folder, preset, cook, &[CellLevels::On, CellLevels::Off])?;
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
    copy_into_bin(folder, &sandbox, SANDBOX)?;
    build_testkit(folder)?;

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

/// Puts the content at `preset` (the shipped cook of `content/` for `ci`, past it the shipped content with the
/// kinds) and the preset's district for `cook` together in [`content_folder`], once for each of `cells`.
fn put_together(folder: &Path, preset: Preset, cook: Cook, cells: &[CellLevels]) -> anyhow::Result<()> {
    // Held, each under its lock, while the districts are put beside it.
    let kinds = bench::has_kinds(preset).then(|| bench::cook_kinds(preset, cook)).transpose()?;
    let shipped = if kinds.is_none() { Some(content::cook(cook)?) } else { None };
    let content_dir = match (&kinds, &shipped) {
        (Some(kinds), _) => &kinds.dir,
        (None, Some(shipped)) => &shipped.dir,
        (None, None) => unreachable!("one of the two is cooked"),
    };
    for &cells in cells {
        let district = bench::cook_with(preset, cook, cells)?;
        let out = content_folder(folder, preset, cook, cells);
        bench::put_beside(content_dir, &district.dir, &out, cook)?;
        eprintln!(
            "put the content and the {} district{} together for {cook} in {}",
            preset.name(),
            if cells == CellLevels::Off { " (its cells with one level)" } else { "" },
            out.display()
        );
    }
    Ok(())
}

/// Builds the testkit and copies it into `bin/`, where every command but `prepare`'s cooks runs it from.
fn build_testkit(folder: &Path) -> anyhow::Result<PathBuf> {
    let env = platforms::host_build_env()?;
    let built = workspace::build_binary(
        &CargoBuild { package: TESTKIT, target: None, profile: BuildProfile::Dev, features: &[], env: &env },
        TESTKIT,
    )?;
    copy_into_bin(folder, &built, TESTKIT)
}

/// Copies a built binary into the batch's `bin/` as `name`, making the folder first: whichever binary a
/// command copies first finds none on a commit that was never prepared.
fn copy_into_bin(folder: &Path, built: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let to = bin(folder, name);
    fs::create_dir_all(folder.join("bin"))?;
    fs::copy(built, &to).with_context(|| format!("copying {} to {}", built.display(), to.display()))?;
    Ok(to)
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

/// `content/<preset>/<cook>[-no-cell-levels]/`: what a run on `cook`'s tiers mounts, the cells with their levels
/// or with one. The testkit finds the second beside the first (`Variant::content`).
fn content_folder(folder: &Path, preset: Preset, cook: Cook, cells: CellLevels) -> PathBuf {
    folder.join("content").join(preset.name()).join(format!("{}{}", cook.name(), cells.suffix()))
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

    /// D6's pair (M5 Z2): the content without the cells' levels sits beside each cook's, its name the cook's with
    /// the suffix a manifest's variant names (`content: Some("no-cell-levels")`).
    #[test]
    fn the_content_without_cell_levels_sits_beside_each_cooks() {
        let folder = Path::new("batch");
        let with = content_folder(folder, Preset::Budget, Cook::Mobile, CellLevels::On);
        let without = content_folder(folder, Preset::Budget, Cook::Mobile, CellLevels::Off);
        assert_eq!(with, Path::new("batch/content/budget/mobile"));
        assert_eq!(without, Path::new("batch/content/budget/mobile-no-cell-levels"));
        let manifest = fs::read_to_string(workspace::root().join(MANIFEST)).expect("the committed manifest");
        assert!(manifest.contains(&format!("content: Some(\"{}\")", &CellLevels::Off.suffix()[1..])));
    }

    /// `prepare` copies the sandbox before the testkit, into a commit's folder that has no `bin/` yet.
    #[test]
    fn a_binary_is_copied_into_a_folder_with_no_bin_yet() {
        let root = std::env::temp_dir().join(format!("xtask_copy_into_bin_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let folder = root.join("batch");
        fs::create_dir_all(&folder).unwrap();
        let built = root.join("built");
        fs::write(&built, "a binary").unwrap();
        let to = copy_into_bin(&folder, &built, SANDBOX).unwrap();
        assert_eq!(to, bin(&folder, SANDBOX));
        assert_eq!(fs::read_to_string(&to).unwrap(), "a binary");
        // And again beside it, as the testkit follows the sandbox.
        let second = copy_into_bin(&folder, &built, TESTKIT).unwrap();
        assert_eq!(fs::read_to_string(second).unwrap(), "a binary");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_manifests_preset_is_read_from_its_text() {
        let text = "// preset: \"double\" is in a comment\n(\n    data: (\n        preset: \"budget\",\n";
        assert_eq!(preset_in(text), Some("budget"));
        assert_eq!(preset_in("(data: ())"), None);
        let committed = fs::read_to_string(workspace::root().join(MANIFEST)).expect("the committed manifest");
        assert!(preset_in(&committed).is_some_and(|name| name.parse::<Preset>().is_ok()));
    }
}
