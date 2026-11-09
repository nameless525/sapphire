//! The replay runner's command line: see the library's docs. `cargo xtask replay` runs it.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context as _, bail};
use sapphire_core::alloc::CountingAllocator;
use sapphire_gameplay::replay::{ARCHITECTURE, Scenario};
use sapphire_replay::{Record, Suite, check, list, record, values_help};
use sapphire_script::scenarios::ALL;

/// What gate 4 reads: the suite refuses to run without it. Every allocation in this process counts,
/// and the gate looks only at the simulation thread's, inside its steps.
#[global_allocator]
static GLOBAL: CountingAllocator<std::alloc::System> = CountingAllocator::new(std::alloc::System);

const USAGE: &str = "\
Usage:
  sapphire_replay check --dir <replays> --out <dir> [--update] [--replay <name>]
  sapphire_replay record <name> --dir <replays> --out <dir> --scenario <name> --script <name>
                         [--ticks <n>] [--seed <n>]
  sapphire_replay list --dir <replays> --out <dir>";

/// `--name value` pairs and bare flags, with anything else an error when they're all taken.
struct Args(Vec<String>);

impl Args {
    fn flag(&mut self, name: &str) -> bool {
        let found = self.0.iter().position(|arg| arg == name);
        found.map(|index| self.0.remove(index)).is_some()
    }

    fn value(&mut self, name: &str) -> anyhow::Result<Option<String>> {
        let Some(index) = self.0.iter().position(|arg| arg == name) else { return Ok(None) };
        anyhow::ensure!(index + 1 < self.0.len(), "{name} needs a value");
        self.0.remove(index);
        Ok(Some(self.0.remove(index)))
    }

    fn required(&mut self, name: &str) -> anyhow::Result<String> {
        self.value(name)?.with_context(|| format!("{name} is required"))
    }

    fn number<T: std::str::FromStr>(&mut self, name: &str) -> anyhow::Result<Option<T>> {
        let Some(value) = self.value(name)? else { return Ok(None) };
        let number = value.parse().map_err(|_| anyhow::anyhow!("{name} expects a number, got {value:?}"))?;
        Ok(Some(number))
    }

    fn finish(self) -> anyhow::Result<()> {
        match self.0.first() {
            Some(arg) => bail!("unexpected argument `{arg}`"),
            None => Ok(()),
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<ExitCode> {
    let mut args = Args(std::env::args().skip(1).collect());
    anyhow::ensure!(!args.0.is_empty(), "no command given\n\n{USAGE}");
    // Gameplay's list can't name a script, which stands above it.
    let scenarios: &[Scenario] = &ALL;
    let command = args.0.remove(0);
    let name = (command == "record" && !args.0.is_empty()).then(|| args.0.remove(0));
    let dir = PathBuf::from(args.required("--dir")?);
    let out = PathBuf::from(args.required("--out")?);
    match command.as_str() {
        "check" => {
            let update = args.flag("--update");
            let only = args.value("--replay")?;
            args.finish()?;
            let outcome = check(&Suite {
                dir: &dir,
                out: &out,
                update,
                only: only.as_deref(),
                scenarios,
                architecture: ARCHITECTURE,
            })?;
            if outcome.failures.is_empty() {
                return Ok(ExitCode::SUCCESS);
            }
            for failure in &outcome.failures {
                eprintln!("FAILED {failure}");
            }
            eprintln!("{}", values_help(&outcome, &out, ARCHITECTURE));
            eprintln!("{} failures on {ARCHITECTURE}", outcome.failures.len());
            Ok(ExitCode::FAILURE)
        }
        "record" => {
            let name = name.with_context(|| format!("record needs a name\n\n{USAGE}"))?;
            let scenario = args.required("--scenario")?;
            let script = args.required("--script")?;
            let ticks = args.number("--ticks")?;
            let seed = args.number("--seed")?;
            args.finish()?;
            record(&Record {
                dir: &dir,
                out: &out,
                name: &name,
                scenario: &scenario,
                script: &script,
                ticks,
                seed,
                scenarios,
                architecture: ARCHITECTURE,
            })?;
            Ok(ExitCode::SUCCESS)
        }
        "list" => {
            args.finish()?;
            list(&dir, scenarios)?;
            Ok(ExitCode::SUCCESS)
        }
        other => bail!("unknown command `{other}`\n\n{USAGE}"),
    }
}
