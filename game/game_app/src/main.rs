//! Desktop and iOS entry point. Android enters through the `SDL_main` exported by the library, which
//! picks the windowed or the offscreen game the same way, by the launching intent's arguments.
//!
//! A package's build (`cargo xtask package`, which the build script sees) is a GUI program on Windows, so
//! nothing opens a console beside the game; the attribute means nothing on other targets.

#![cfg_attr(sapphire_packaged, windows_subsystem = "windows")]

use std::io::Write as _;
use std::process::ExitCode;

use game_app::{GameApp, OffscreenGame};
use sapphire_platform::RunOptions;

fn main() -> ExitCode {
    // Must come first: when this process was relaunched as the crash monitor, it serves
    // minidump requests here and never returns.
    sapphire_crash::run_monitor_if_requested();
    // `--version`: one line, before logging, crash capture or SDL.
    if let Some(line) = game_app::version_requested() {
        return print_line(&line);
    }
    game_app::startup();
    let exit = if game_app::offscreen_requested() {
        // `--capture`: no window or display at all, and the process never comes to the foreground.
        sapphire_platform::run_with::<OffscreenGame>(RunOptions { background: true, headless: true })
    } else {
        sapphire_platform::run::<GameApp>()
    };
    game_app::shutdown();
    exit
}

/// Writes `line` to stdout: a closed pipe (`| head`) isn't a failure.
fn print_line(line: &str) -> ExitCode {
    let result = {
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{line}").and_then(|()| stdout.flush())
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(std::io::stderr().lock(), "could not write the version: {error}");
            ExitCode::FAILURE
        }
    }
}
