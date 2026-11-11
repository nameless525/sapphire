//! Desktop and iOS entry point. Android enters through the `SDL_main` exported by the library, which
//! picks the windowed or the offscreen game the same way, by the launching intent's arguments.
//!
//! A package's build (`cargo xtask package`, which the build script sees) is a GUI program on Windows, so
//! nothing opens a console beside the game; the attribute means nothing on other targets.

#![cfg_attr(sapphire_packaged, windows_subsystem = "windows")]

use std::process::ExitCode;

use game_app::{GameApp, OffscreenGame};
use sapphire_platform::RunOptions;

fn main() -> ExitCode {
    // Must come first: when this process was relaunched as the crash monitor, it serves
    // minidump requests here and never returns.
    sapphire_crash::run_monitor_if_requested();
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
