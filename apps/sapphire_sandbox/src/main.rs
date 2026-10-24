//! Desktop and iOS entry point. Android enters through the `SDL_main` exported by the library.

use std::process::ExitCode;

use sapphire_sandbox::SandboxApp;

fn main() -> ExitCode {
    // Must come first: when this process was relaunched as the crash monitor, it serves
    // minidump requests here and never returns.
    sapphire_crash::run_monitor_if_requested();
    sapphire_sandbox::startup();
    let exit = sapphire_platform::run::<SandboxApp>();
    sapphire_sandbox::shutdown();
    exit
}
