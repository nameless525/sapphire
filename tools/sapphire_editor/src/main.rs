//! `sapphire_editor`: the combat timeline editor's binary. See the library's docs.
//!
//! `--capture` and `--run` never touch SDL. Anything else opens a window, so agents only ever run this with
//! `--capture` or `--run` (`docs/development-iteration.md`, "The timeline editor").

mod window;

use std::process::ExitCode;

use sapphire_editor::cli::{self, Launch};

fn main() -> ExitCode {
    if let Err(error) = sapphire_core::log::init(sapphire_core::log::LogConfig::default(), None) {
        eprintln!("logging setup failed: {error}");
    }
    let line: Vec<String> = std::env::args().collect();
    match cli::parse_command_line(&line) {
        Ok(Launch::Capture(options, capture)) => done(cli::run_capture(options, &capture)),
        Ok(Launch::Run(options, run)) => done(cli::run_script(options, &run)),
        Ok(Launch::Window { .. }) => sapphire_platform::run::<window::EditorApp>(),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// What a run with no window comes to.
fn done(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
