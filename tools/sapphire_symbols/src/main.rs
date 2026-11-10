//! The symbol checker's command line: see the library's docs. `cargo xtask package` runs it.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
Usage:
  sapphire_symbols check --shipped <binary> --symbols <dSYM, unstripped ELF or PDB>

Prints the module the two make as one line of JSON: format, arch, debug_file, debug_id and code_id.";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> anyhow::Result<()> {
    let [command, first, first_value, second, second_value] = args.as_slice() else {
        anyhow::bail!("{USAGE}");
    };
    let (shipped, symbols) = match (first.as_str(), second.as_str()) {
        ("--shipped", "--symbols") => (first_value, second_value),
        ("--symbols", "--shipped") => (second_value, first_value),
        _ => anyhow::bail!("{USAGE}"),
    };
    anyhow::ensure!(command == "check", "{USAGE}");
    let module = sapphire_symbols::check(&PathBuf::from(shipped), &PathBuf::from(symbols))?;
    writeln!(std::io::stdout().lock(), "{}", module.to_json())?;
    Ok(())
}
