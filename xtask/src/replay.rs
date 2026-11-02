//! `cargo xtask replay`: the recorded combat replays (spec §9, gates 2 and 4).
//!
//! Builds `sapphire_replay`, which links the simulation and nothing with a GPU or a window, and runs
//! it on `tests/replays/`. It runs as a process because nothing may depend on a tool crate (rule R1),
//! as with `sapphire_testkit`. The build takes no SDK or NDK environment: none of what it links
//! compiles C, which `cargo xtask check-layers` keeps true.

use std::process::Command as Process;

use crate::cli::Args;
use crate::workspace::{self, BuildProfile, CargoBuild};

const RUNNER: &str = "sapphire_replay";

/// What `cargo xtask replay` was asked to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Check { update: bool, replay: Option<String> },
    Record { name: String, scenario: String, script: String, ticks: Option<u32>, seed: Option<u64> },
    List,
}

impl Command {
    pub fn parse(args: &mut Args) -> anyhow::Result<Self> {
        let update = args.flag("--update");
        let list = args.flag("--list");
        let replay = args.value::<String>("--replay")?;
        let record = args.value::<String>("--record")?;
        let scenario = args.value::<String>("--scenario")?;
        let script = args.value::<String>("--script")?;
        let ticks = args.value::<u32>("--ticks")?;
        let seed = args.value::<u64>("--seed")?;

        if let Some(name) = record {
            anyhow::ensure!(!update && !list && replay.is_none(), "--record goes alone");
            let scenario = scenario.ok_or_else(|| anyhow::anyhow!("--record needs --scenario"))?;
            let script = script.ok_or_else(|| anyhow::anyhow!("--record needs --script"))?;
            return Ok(Self::Record { name, scenario, script, ticks, seed });
        }
        anyhow::ensure!(
            scenario.is_none() && script.is_none() && ticks.is_none() && seed.is_none(),
            "--scenario, --script, --ticks and --seed need --record"
        );
        if list {
            anyhow::ensure!(!update && replay.is_none(), "--list goes alone");
            return Ok(Self::List);
        }
        Ok(Self::Check { update, replay })
    }
}

pub fn run(command: &Command) -> anyhow::Result<()> {
    let runner = workspace::build_binary(
        &CargoBuild { package: RUNNER, target: None, profile: BuildProfile::Dev, features: &[], env: &[] },
        RUNNER,
    )?;
    let mut process = Process::new(runner);
    match command {
        Command::Check { update, replay } => {
            process.arg("check");
            if *update {
                process.arg("--update");
            }
            if let Some(name) = replay {
                process.args(["--replay", name]);
            }
        }
        Command::Record { name, scenario, script, ticks, seed } => {
            process.args(["record", name, "--scenario", scenario, "--script", script]);
            if let Some(ticks) = ticks {
                process.arg("--ticks").arg(ticks.to_string());
            }
            if let Some(seed) = seed {
                process.arg("--seed").arg(seed.to_string());
            }
        }
        Command::List => {
            process.arg("list");
        }
    }
    process
        .arg("--dir")
        .arg(workspace::root().join("tests").join("replays"))
        .arg("--out")
        .arg(workspace::output_dir().join("replays"));
    workspace::run(&mut process)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> anyhow::Result<Command> {
        let mut args = Args::new(line.split_whitespace().map(str::to_owned));
        let command = Command::parse(&mut args)?;
        args.finish(false)?;
        Ok(command)
    }

    #[test]
    fn it_runs_records_and_lists() {
        assert_eq!(parse("").unwrap(), Command::Check { update: false, replay: None });
        assert_eq!(
            parse("--update --replay duel").unwrap(),
            Command::Check { update: true, replay: Some("duel".into()) }
        );
        assert_eq!(parse("--list").unwrap(), Command::List);
        assert_eq!(
            parse("--record duel --scenario drill --script idle --ticks 60 --seed 3").unwrap(),
            Command::Record {
                name: "duel".into(),
                scenario: "drill".into(),
                script: "idle".into(),
                ticks: Some(60),
                seed: Some(3),
            }
        );
    }

    #[test]
    fn it_refuses_what_it_cant_mean() {
        for line in [
            "--record duel",
            "--record duel --scenario drill",
            "--record duel --script idle",
            "--record duel --scenario drill --script idle --update",
            "--scenario drill",
            "--ticks 60",
            "--list --update",
            "--updaet",
        ] {
            assert!(parse(line).is_err(), "{line}");
        }
    }
}
