//! Desktop and iOS entry point. Android enters through the `SDL_main` exported by the library.

use std::process::ExitCode;

use sapphire_platform::RunOptions;
use sapphire_sandbox::{OffscreenApp, SandboxApp};

fn main() -> ExitCode {
    // Must come first: when this process was relaunched as the crash monitor, it serves
    // minidump requests here and never returns.
    sapphire_crash::run_monitor_if_requested();
    sapphire_sandbox::startup();
    let exit = if sapphire_sandbox::offscreen_requested() {
        // Capture or bench: no window or display at all, and the process never comes to the
        // foreground.
        sapphire_platform::run_with::<OffscreenApp>(RunOptions { background: true, headless: true })
    } else {
        sapphire_platform::run::<SandboxApp>()
    };
    sapphire_sandbox::shutdown();
    exit
}
