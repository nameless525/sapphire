//! The game: its flow from the title through its runs, with the game's menus (M3 S2, U7, G1).
//!
//! Everything it draws is `sapphire_stage`'s and everything it plays `game_flow`'s, `game_combat`'s and
//! `game_ui`'s: what stands on the stage is `game_flow`'s [`GameShow`], which the sandbox's `--game` stands
//! too. This is the app around it: the window's frame (`app.rs`), its flags (`options.rs`), where the player's
//! settings live (`settings.rs`) and what mobile links. It has no scene-building flags: those are the
//! sandbox's, which stays the bench and the goldens' host.
//!
//! The flow is the one the content names (`content/game.documents.ron`'s `flow`); content that names none
//! plays the arena's fight alone, as before G1. The save is in the platform's directory for the player's
//! files.
//!
//! # Controls
//!
//! A menu that's up has every press (`game_ui::menus`, `sapphire_controls`' `MenuControls`): the title's new
//! game starts a run from the scenario's start, its continue from where the save says, and Back or `Esc`
//! leaves. The pause goes back to the title, under the wipe. In a run, it's combat's bindings
//! (`sapphire_controls`' `combat_bindings.ron`), the HUD's touch zones and its pause button; what pauses
//! opens the pause's screen.
//!
//! # Arguments
//!
//! For runs nobody's playing:
//! - `--new-game` or `--continue`: past the title at once, as its row would. `--open-fight` is `--new-game`.
//! - `--script <name>`: each run plays this script, not whoever's there, and logs how it's going.
//! - `--save-dir <dir>`: the save's kept here. A capture keeps one only where this says.
//! - `--record <dir>`: each run's replay is kept here, `<scenario>-<n>.sreplay`.
//! - `--capture <png>`, with `--size <w>x<h>` and `--ticks <n>`: draws that many steps with no window, writes
//!   the last frame and exits.
//! - `--lose-device <frames>` (dev builds): destroys the GPU device once, after that many frames presented, so
//!   the rebuild below runs where nothing loses a device when asked.
//! - `--crash panic|fault` (every build, the store's too): crashes as soon as crash capture is installed, so
//!   `cargo xtask symbolicate` can show the crash read back (M4 C2). On Android, where a store build takes no
//!   launch arguments, `adb shell setprop debug.sapphire.crash panic|fault` asks instead.
//!
//! # Leaving the foreground, and a lost device (M4 S6)
//!
//! A trip to the background pauses a run someone's playing, writes the save, the settings and a kept run's
//! file, and suspends the audio; the return lands on the pause's screen, the "tap to continue" (`app.rs`). A
//! lost device takes the stage: the game boots again on a new device, under the boot screen (M4 H2), and goes
//! back where its save has it, paused if someone was playing (`game_flow`'s `Boot` and `Reload`; a test stands
//! it at once with [`stand_game`]).
//!
//! # What it doesn't do yet
//!
//! It stays on the tier it started on. A lost device costs what was played since the save.

mod app;
mod offscreen;
mod options;
mod settings;

/// What this build of the game is, compiled in by the build script: the bundle ID from
/// `platform/identity.toml`, and the build number and ID `cargo xtask package` gave it. The patch manifest
/// reads [`BUILD_NUMBER`](build_info::BUILD_NUMBER) as the build this binary is.
pub mod build_info {
    include!(concat!(env!("OUT_DIR"), "/build_info.rs"));
}

use std::sync::{Mutex, Once};

use game_combat::combat::CombatOption;
use game_combat::fight::fight_stage;
use sapphire_core::alloc::EngineAllocator;
use sapphire_core::log::{LogConfig, LogSink};
use sapphire_crash::{CrashConfig, CrashGuard, ForcedCrash};
use sapphire_platform::{App, AppInitError, DeviceClass, Host, Os, PlatformLogSink};
use sapphire_render::quality::QualitySettings;
use sapphire_rhi::{GpuContext, wgpu};
use sapphire_stage::{Content, Stage, StageDesc};

pub use app::GameApp;
pub use game_flow::show::GameShow;
pub use offscreen::OffscreenGame;

/// mimalloc, counted so the R6 check can see allocations.
#[global_allocator]
static GLOBAL: EngineAllocator = sapphire_core::alloc::engine_allocator();

/// Log target for the game's app.
pub(crate) const LOG_TARGET: &str = "game::app";

/// Crash capture for the whole process: installed by [`startup`], released by [`shutdown`].
static CRASH_GUARD: Mutex<Option<CrashGuard>> = Mutex::new(None);

/// Process setup that runs before SDL starts, in the order `sapphire_crash` requires: logging, crash
/// capture, a crash if one's asked for (`--crash`, or Android's `debug.sapphire.crash`), then the job system
/// (spec §5.2). Called from `main` on desktop and iOS (after
/// `sapphire_crash::run_monitor_if_requested`), and from `SDL_main` on Android, where it must not panic. Only
/// the first call does anything.
pub fn startup() {
    static STARTUP: Once = Once::new();
    STARTUP.call_once(|| {
        // Mobile has no visible stderr: route log lines to logcat / the unified log.
        let mobile = Os::CURRENT.device_class() == DeviceClass::Mobile;
        let config = LogConfig { stderr: !mobile, ..LogConfig::default() };
        let sink = mobile.then(|| Box::new(PlatformLogSink) as Box<dyn LogSink>);
        if let Err(error) = sapphire_core::log::init(config, sink) {
            sapphire_platform::sdl_log(tracing::Level::WARN, &format!("logging setup failed: {error}"));
        }
        let guard = install_crash_capture();
        *CRASH_GUARD.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = guard;
        match ForcedCrash::requested() {
            Ok(Some(crash)) => crash.now(),
            Ok(None) => {}
            Err(error) => tracing::warn!(target: LOG_TARGET, %error, "no crash forced"),
        }
        sapphire_jobs::init(sapphire_jobs::JobConfig::default());
    });
}

/// The game's `show` stood on a stage on `gpu`, drawing into `format` targets, at its title with `first` the
/// run behind it, in one go: what a test does with no window. The window boots it over frames instead, under
/// the boot screen (`game_flow::boot`, M4 H2), at startup and again on a new device after one's lost (M4 S6). Fails if the device is lost while it stands, rather than hand back a stage of
/// dead objects.
pub fn stand_game(
    host: &Host,
    gpu: GpuContext,
    format: wgpu::TextureFormat,
    mut show: GameShow,
    first: &CombatOption,
) -> Result<Stage<GameShow>, AppInitError> {
    let desc = fight_stage(StageDesc::new(host, &gpu, format, QualitySettings::default()));
    let mut content = Content::new(host, desc.character, desc.sky)?;
    let combat = show.shown.fight.prepare_stock_combat(first, &mut content)?;
    let mut stage = Stage::new_with_content(gpu, desc, content)?.with_show(show);
    let Stage { show, scene, content, renderer, gpu, quality, .. } = &mut stage;
    show.stand(combat, scene, content, renderer, gpu, quality.active().id)?;
    // Uploads to a lost device do nothing, so what stood on it holds dead objects.
    if stage.gpu.is_lost() {
        return Err(AppInitError::msg("the GPU device was lost while the game stood"));
    }
    Ok(stage)
}

/// Whether the arguments ask for [`OffscreenGame`] in the background, not the window.
pub fn offscreen_requested() -> bool {
    std::env::args_os().any(|arg| arg.to_str().is_some_and(options::is_offscreen_flag))
}

/// Whether `args` ask for [`OffscreenGame`]: [`offscreen_requested`] for arguments that aren't the
/// process's. Android's `SDL_main` asks it of the launching intent's (M3 Z2b), so a capture runs there too.
pub fn asks_offscreen(args: &[String]) -> bool {
    args.iter().any(|arg| options::is_offscreen_flag(arg))
}

/// Releases process-wide resources once the platform loop has returned (desktop). iOS never returns from
/// `run`, and Android keeps the process alive, so neither calls it.
pub fn shutdown() {
    drop(CRASH_GUARD.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take());
}

/// Installs crash capture into the app's cache directory, which is never backed up. Failure leaves the app
/// running without it.
fn install_crash_capture() -> Option<CrashGuard> {
    let metadata = GameApp::METADATA;
    let dir = match sapphire_platform::cache_dir(&metadata) {
        Ok(dir) => dir.join("crashes"),
        Err(error) => {
            tracing::warn!(target: LOG_TARGET, %error, "no cache directory; crash capture off");
            return None;
        }
    };
    match sapphire_crash::install(CrashConfig::new(metadata.name, build_info::BUILD_ID, dir)) {
        Ok(guard) => Some(guard),
        Err(error) => {
            tracing::warn!(target: LOG_TARGET, %error, "crash capture unavailable");
            None
        }
    }
}

sapphire_platform::export_android_main!(
    GameApp,
    init = startup,
    offscreen = OffscreenGame,
    when = asks_offscreen
);

#[cfg(test)]
mod tests {
    use super::asks_offscreen;

    /// Android's entry picks the app by this: a capture runs offscreen and ends, and anything else is
    /// the window's.
    #[test]
    fn an_intent_asking_for_a_capture_runs_offscreen() {
        let args = |list: &[&str]| list.iter().map(|&arg| arg.to_owned()).collect::<Vec<_>>();
        assert!(asks_offscreen(&args(&["--open-fight", "--capture", "fight.png", "--ticks", "600"])));
        assert!(asks_offscreen(&args(&["--capture=title.png"])));
        assert!(!asks_offscreen(&args(&["--open-fight", "--script", "fight-back"])));
        assert!(!asks_offscreen(&[]));
    }
}
