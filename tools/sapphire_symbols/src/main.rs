//! The command line of `sapphire_symbols`: see the library's docs. `cargo xtask package` and
//! `cargo xtask symbolicate` run it.

use std::io::{BufWriter, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context as _, bail, ensure};
use sapphire_symbols::crash::{self, PathMap};
use sapphire_symbols::store::{self, Store};

const USAGE: &str = "\
Usage:
  sapphire_symbols check --shipped <binary> --symbols <dSYM, unstripped ELF or PDB>
      Prints the module the two make as one line of JSON: format, arch, debug_file, debug_id and code_id.
  sapphire_symbols dump --shipped <binary> --symbols <dSYM, unstripped ELF or PDB>
      Prints the module's Breakpad .sym, after the same check.
  sapphire_symbols symbolicate --store <dir> [--package <dir>]... [--map <shipped>=<local>]... <report>...
      Reads each crash report with the symbols the packages kept, and prints its frames.
  sapphire_symbols store --store <dir> --package <dir>...
      Writes the Breakpad .sym of every module the packages shipped into the store, and prints each path.
  sapphire_symbols scan [--forbid <path>]... <file or directory>...
      Fails if any file holds a forbidden path, with either kind of slash.";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Options in any order, then the rest.
struct Args {
    options: Vec<(String, String)>,
    rest: Vec<String>,
}

impl Args {
    fn parse(args: &[String], names: &[&str]) -> anyhow::Result<Self> {
        let (mut options, mut rest) = (Vec::new(), Vec::new());
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            if names.contains(&arg.as_str()) {
                let value = args.next().with_context(|| format!("{arg} needs a value\n\n{USAGE}"))?;
                options.push((arg.clone(), value.clone()));
            } else if arg.starts_with("--") {
                bail!("unknown option {arg}\n\n{USAGE}");
            } else {
                rest.push(arg.clone());
            }
        }
        Ok(Self { options, rest })
    }

    fn all(&self, name: &str) -> impl Iterator<Item = &str> {
        self.options.iter().filter(move |(option, _)| option == name).map(|(_, value)| value.as_str())
    }

    fn one(&self, name: &str) -> anyhow::Result<PathBuf> {
        match self.all(name).collect::<Vec<_>>().as_slice() {
            [value] => Ok(PathBuf::from(value)),
            _ => bail!("give {name} once\n\n{USAGE}"),
        }
    }
}

fn run(args: Vec<String>) -> anyhow::Result<()> {
    let Some((command, args)) = args.split_first() else { bail!("{USAGE}") };
    let stdout = std::io::stdout();
    match command.as_str() {
        "check" | "dump" => {
            let args = Args::parse(args, &["--shipped", "--symbols"])?;
            ensure!(args.rest.is_empty(), "{USAGE}");
            let symbols = args.one("--symbols")?;
            let module = sapphire_symbols::check(&args.one("--shipped")?, &symbols)?;
            let mut out = BufWriter::new(stdout.lock());
            if command == "check" {
                writeln!(out, "{}", module.to_json())?;
            } else {
                sapphire_symbols::sym::write(&module, &symbols, &mut out)?;
            }
            out.flush()?;
        }
        "symbolicate" => {
            let args = Args::parse(args, &["--store", "--package", "--map"])?;
            ensure!(!args.rest.is_empty(), "symbolicate needs a report\n\n{USAGE}");
            let packages: Vec<PathBuf> = args.all("--package").map(PathBuf::from).collect();
            let packaged = store::packaged(&packages)?;
            let store = Store { root: args.one("--store")? };
            let maps: Vec<PathMap> = args.all("--map").map(PathMap::parse).collect::<anyhow::Result<_>>()?;
            for report in &args.rest {
                let crash = crash::read(report.as_ref(), &packaged, &store)?;
                writeln!(stdout.lock(), "{report}\n{}", crash::render(&crash, &maps))?;
            }
        }
        "store" => {
            let args = Args::parse(args, &["--store", "--package"])?;
            ensure!(args.rest.is_empty(), "{USAGE}");
            let packages: Vec<PathBuf> = args.all("--package").map(PathBuf::from).collect();
            ensure!(!packages.is_empty(), "store needs a package\n\n{USAGE}");
            let store = Store { root: args.one("--store")? };
            let mut out = BufWriter::new(stdout.lock());
            for path in store.fill(&store::packaged(&packages)?)? {
                writeln!(out, "{}", path.display())?;
            }
            out.flush()?;
        }
        "scan" => {
            let args = Args::parse(args, &["--forbid"])?;
            ensure!(!args.rest.is_empty(), "scan needs a file or directory\n\n{USAGE}");
            let paths: Vec<PathBuf> = args.rest.iter().map(PathBuf::from).collect();
            let forbidden: Vec<String> = args.all("--forbid").map(str::to_owned).collect();
            let found = sapphire_symbols::scan::scan(&paths, &forbidden)?;
            for found in &found {
                eprintln!(
                    "{} holds {} at {:#x}: {}",
                    found.file.display(),
                    found.forbidden,
                    found.offset,
                    found.context
                );
            }
            ensure!(found.is_empty(), "{} places hold this machine's paths", found.len());
        }
        _ => bail!("{USAGE}"),
    }
    Ok(())
}
