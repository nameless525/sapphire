//! Sapphire Sandbox, the engine test bed (milestone M0, spec §10).
//!
//! It runs the whole engine path on every platform: the SDL3 platform layer, the fixed 60 Hz
//! simulation on the engine clocks and ECS, time of day (`sapphire_world::tod`) resolving the
//! frame's look and the character's light rig, plain-data
//! [`FramePacket`](sapphire_render::FramePacket)s filled by
//! change-detecting extract systems, and the wgpu anime renderer's persistent scene, drawing the
//! cooked sample mannequin.
//!
//! # Controls
//!
//! The default bindings are a `sapphire_input` document, `default_bindings.ron`, compiled into
//! the app (see `input.rs`).
//!
//! | Input | Action |
//! |---|---|
//! | Left-drag, one-finger drag, right stick, arrow keys | Orbit |
//! | Wheel, pinch, triggers | Zoom |
//! | `Space`, gamepad South, double tap | Toggle world slow motion (perfect-dodge style, 0.2×) |
//! | `H`, gamepad East, long press (0.5 s) | 8-tick hitstop on the turntable |
//! | `Esc` | Quit (desktop) |
//! | `` ` ``, a still two-finger hold (1 s) | Show or hide the debug overlay (`dev` builds) |
//!
//! # Development builds
//!
//! The `dev` feature links `sapphire_debug` and `sapphire_hot_reload` (rule R8), all of it
//! behind `dev.rs`: the debug overlay, which sees platform events before the game's input,
//! applies its time and time-of-day controls before each fixed step and draws after post; the
//! cvar registry it edits, holding the sandbox's cvars (`cvars.rs`, compiled in every build so
//! they read their defaults) and the renderer's quality cvars (`r.tier`, `r.render_scale`,
//! `r.dynamic_resolution`); and hot reload (`reload.rs`, M1 A8), on by default on desktop:
//! rewritten packs in the content root are remounted and their changed assets replaced in
//! place, edited WGSL rebuilds the renderer's pipelines, an edited `default_day.ron` replaces
//! time of day's timeline and an edited `default_light_rigs.ron` the character light rig
//! presets, all without a restart.
//! Without the feature none of it is compiled.
//!
//! # Arguments
//!
//! - `--frames N`: exit after N presented frames, for unattended runs; fails if they can't be
//!   presented in time.
//! - `--tier desktop-high|desktop-low|mobile-high|mobile-low`: the player's quality tier
//!   (`sapphire_render::quality`) instead of the one selected for the device. The `r.tier` cvar
//!   overrides it in `dev` builds, and a change makes a new renderer.
//! - `--render-scale <0.25..1>`: the player's base render scale instead of the tier's.
//! - `--dynamic-resolution`: let dynamic resolution lower the render scale to hold the tier's
//!   GPU budget (off by default; `r.dynamic_resolution` overrides it).
//! - `--aa off|msaa|taa`: the antialiasing technique instead of the tier's (temporal AA on the
//!   desktop tiers, 4× MSAA on mobile ones; M1 R7): `msaa` is the tier's sample count or 4×,
//!   `taa` turns MSAA off. For comparisons and golden images.
//! - `--msaa off|2|4|8`: override the sample count the tier or `--aa` gives.
//! - `--spin <radians per second>`: the turntable's speed (default 0.35), for edges in motion.
//! - `--lights`: place three colored local lights around the character.
//! - `--skinned`: place a procedural skinned and morphed stalk, swaying, beside the character
//!   (compute skinning, M1 R2).
//! - `--swatches`: place three spheres on the character's other side that show the anime
//!   character shader's diffuse modes over cooked maps (ramp mode, ramp mode with a subsurface
//!   mix, three bands; M1 C1).
//! - `--faces`: show the face look-dev rig instead of the character: rows of the face test head,
//!   each a character lit by a key light of its own from an angle in its head space (a yaw sweep
//!   across both sides, top light), and two with blush and gloom (M1 C3).
//! - `--light-path clustered|per-object`: override how the tier lists local lights (clustered
//!   on desktop tiers, per-object on mobile ones).
//! - `--time-of-day <hours>`: start the day at this hour, `0 ≤ hours < 24` (default 10, the
//!   default day's key that is exactly the look from before time of day). Time stands still
//!   unless `--day-length` runs it.
//! - `--day-length <seconds>`: run the day, one every this many seconds of world time (so
//!   pause and slow motion reach it).
//! - `--character-rig <preset>`: light the character with this character light rig preset
//!   (`sapphire_world::light_rig`; `gameplay`, the default, or `portrait`).
//! - `--cloud-cover <0..1>`: hold both cloud layers at this coverage instead of time of day's;
//!   `0` is a clear sky.
//! - `--plain-sky`: draw the sky's gradient alone (no discs, glows, stars or clouds), a backdrop
//!   the same at any hour, which the golden scenes from before the sky's layers pin.
//! - `--view <yaw>,<pitch>`: start the orbit camera at these angles in degrees (yaw orbits right
//!   from in front of the character; a negative pitch looks up at the sky).
//! - `--no-hot-reload` (`dev` builds): watch nothing, so files edited during the run don't
//!   change it.
//! - `--debug-overlay[=panels]` (`dev` builds): show the overlay from the start, with its
//!   default panels or a comma-separated list (`stats`, `graph`, `time`, `day`, `console`,
//!   `cvars`, `all`). Offscreen captures draw it too.
//! - Offscreen modes (desktop), which never open a window or take focus, at `--size WxH`
//!   (default 1920x1080):
//!   - `--capture <png> [--capture-ticks N]`: render the scene after N fixed ticks (default
//!     90), write a PNG and exit. `--capture-frames F` draws F frames and writes the last, for
//!     temporal AA's history; `--capture-step S` puts S ticks between them (0, the default:
//!     the scene holds still at tick N; 1: it plays at 60 fps up to tick N).
//!   - `--bench <frames>`: run the per-frame CPU path that many times and log CPU time and
//!     allocations per frame, then GPU pass times.
//!
//! Desktop runs need cooked content: `cargo xtask run --platform macos` cooks and launches.
//!
//! # The sky
//!
//! The zone's sky is the sky material `samples/sky.material.ron` (`content::SKY`), loaded with its
//! maps at startup and after every renderer rebuild, and drawn with time of day's sky layers:
//! card clouds on the horizon, cirrus, the sun and moon with their glows, stars and the Milky Way
//! (spec §5.12, "Sky").

mod app;
mod bench;
mod capture;
mod content;
mod cvars;
#[cfg(feature = "dev")]
mod dev;
mod extract;
mod faces;
mod input;
mod offscreen;
mod options;
mod orbit;
#[cfg(feature = "dev")]
mod reload;
mod scene;
mod skinned;
mod stats;
mod swatches;
mod turntable;

use std::cell::RefCell;
use std::fmt::Write as _;
use std::sync::{Mutex, Once};

use sapphire_core::alloc::EngineAllocator;
use sapphire_core::log::{LogConfig, LogLevel, LogRecord, LogSink};
use sapphire_crash::{CrashConfig, CrashGuard};
use sapphire_platform::{App, DeviceClass, Os};

pub use app::SandboxApp;
pub use offscreen::OffscreenApp;

/// mimalloc, counted so the frame stats and the R6 check can see allocations.
#[global_allocator]
static GLOBAL: EngineAllocator = sapphire_core::alloc::engine_allocator();

/// Log target for the sandbox.
pub(crate) const LOG_TARGET: &str = "sapphire::sandbox";

/// Crash capture for the whole process: installed by [`startup`], released by [`shutdown`].
static CRASH_GUARD: Mutex<Option<CrashGuard>> = Mutex::new(None);

/// Process setup that runs before SDL starts, in the order `sapphire_crash` requires:
/// logging, crash capture, then the job system (spec §5.2).
///
/// Called from `main` on desktop and iOS (after `sapphire_crash::run_monitor_if_requested`),
/// and from `SDL_main` on Android, where it must not panic. Android can start `SDL_main`
/// again in the same process after the activity is recreated, so only the first call does
/// anything.
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

        let jobs = sapphire_jobs::init(sapphire_jobs::JobConfig::default());
        tracing::info!(target: LOG_TARGET, topology = ?jobs.topology(), config = ?jobs.config(), "job system ready");
    });
}

/// Whether the arguments ask for an offscreen mode (`--capture` or `--bench`), which `main`
/// runs as [`OffscreenApp`] in the background instead of opening the windowed app.
pub fn offscreen_requested() -> bool {
    std::env::args_os().any(|arg| arg.to_str().is_some_and(options::is_offscreen_flag))
}

/// Releases process-wide resources once the platform loop has returned (desktop). iOS never
/// returns from `run`, and Android keeps the process alive, so neither calls it.
pub fn shutdown() {
    drop(CRASH_GUARD.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take());
}

/// Installs crash capture into the app's cache directory, which is never backed up. Failure
/// leaves the app running without it.
fn install_crash_capture() -> Option<CrashGuard> {
    let metadata = SandboxApp::METADATA;
    let dir = match sapphire_platform::cache_dir(&metadata) {
        Ok(dir) => dir.join("crashes"),
        Err(error) => {
            tracing::warn!(target: LOG_TARGET, %error, "no cache directory; crash capture off");
            return None;
        }
    };
    if let Ok(pending) = sapphire_crash::pending_reports(&dir)
        && !pending.is_empty()
    {
        tracing::warn!(target: LOG_TARGET, count = pending.len(), dir = %dir.display(), "crash reports from earlier runs");
    }
    match sapphire_crash::install(CrashConfig::new(metadata.name, metadata.version, dir)) {
        Ok(guard) => {
            tracing::info!(target: LOG_TARGET, capture = ?guard.native_capture(), run = guard.run_id(), "crash capture on");
            Some(guard)
        }
        Err(error) => {
            tracing::warn!(target: LOG_TARGET, %error, "crash capture unavailable");
            None
        }
    }
}

/// Forwards log records to the platform logger (logcat, the Apple unified log), prefixed
/// with their target.
struct PlatformLogSink;

impl LogSink for PlatformLogSink {
    fn log(&self, record: &LogRecord<'_>) {
        thread_local! {
            // Reused per thread to keep line building cheap.
            static LINE: RefCell<String> = const { RefCell::new(String::new()) };
        }
        let level = tracing_level(record.level);
        // Never panic in a log call: if the buffer is unavailable (logging from inside this
        // sink, or during thread-local teardown), build the line on the spot.
        let written = LINE.try_with(|line| {
            let Ok(mut line) = line.try_borrow_mut() else { return false };
            line.clear();
            let _ = write!(line, "[{}] {}", record.target, record.message);
            sapphire_platform::sdl_log(level, &line);
            true
        });
        if written != Ok(true) {
            sapphire_platform::sdl_log(level, &format!("[{}] {}", record.target, record.message));
        }
    }
}

fn tracing_level(level: LogLevel) -> tracing::Level {
    match level {
        LogLevel::Error => tracing::Level::ERROR,
        LogLevel::Warn => tracing::Level::WARN,
        LogLevel::Info => tracing::Level::INFO,
        LogLevel::Debug => tracing::Level::DEBUG,
        LogLevel::Trace => tracing::Level::TRACE,
    }
}

sapphire_platform::export_android_main!(SandboxApp, init = startup);
