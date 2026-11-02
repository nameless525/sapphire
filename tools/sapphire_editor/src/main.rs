//! `sapphire_editor`: the combat timeline editor's binary. See the library's docs.
//!
//! `--capture` never touches SDL. Anything else opens a window, so agents only ever run this with
//! `--capture` (`docs/development-iteration.md`, "The timeline editor").

mod window;

use std::process::ExitCode;

use sapphire_editor::cli::{self, Command};

fn main() -> ExitCode {
    if let Err(error) = sapphire_core::log::init(sapphire_core::log::LogConfig::default(), None) {
        eprintln!("logging setup failed: {error}");
    }
    let line: Vec<String> = std::env::args().collect();
    match cli::parse_command_line(&line) {
        Ok(Command::Capture(options, capture)) => match cli::run_capture(options, &capture) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        },
        Ok(Command::Window(_)) => sapphire_platform::run::<window::EditorApp>(),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
