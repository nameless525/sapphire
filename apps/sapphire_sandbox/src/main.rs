//! Desktop and iOS entry point. Android enters through the `SDL_main` exported by the library.

use std::io::Write as _;
use std::process::ExitCode;

use sapphire_platform::RunOptions;
use sapphire_sandbox::{OffscreenApp, SandboxApp};

fn main() -> ExitCode {
    // Must come first: when this process was relaunched as the crash monitor, it serves
    // minidump requests here and never returns.
    sapphire_crash::run_monitor_if_requested();
    if let Some(help) = sapphire_sandbox::command_help() {
        let result = {
            let mut stdout = std::io::stdout().lock();
            writeln!(stdout, "{help}").and_then(|()| stdout.flush())
        };
        return match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
            Err(error) => {
                let _ = writeln!(std::io::stderr().lock(), "could not write help: {error}");
                ExitCode::FAILURE
            }
        };
    }
    sapphire_sandbox::startup();
    let exit = if sapphire_sandbox::offscreen_requested() {
        // Capture, bench or `--offscreen`: no window or display at all, and the process never
        // comes to the foreground.
        sapphire_platform::run_with::<OffscreenApp>(RunOptions { background: true, headless: true })
    } else {
        sapphire_platform::run::<SandboxApp>()
    };
    sapphire_sandbox::shutdown();
    exit
}
