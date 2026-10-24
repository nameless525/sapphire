//! Sapphire Sandbox, the engine test bed (milestone M0, spec §10).
//!
//! It runs the whole M0 path on every platform: the SDL3 platform layer, the fixed 60 Hz
//! simulation on the engine clocks, a plain-data [`FrameView`](sapphire_render::FrameView)
//! and the wgpu toon renderer, drawing the cooked sample mannequin.
//!
//! # Controls
//!
//! | Input | Action |
//! |---|---|
//! | Left-drag, one-finger drag, right stick, arrow keys | Orbit |
//! | Wheel, pinch, triggers | Zoom |
//! | `Space`, gamepad South | Toggle world slow motion (perfect-dodge style, 0.2×) |
//! | `H`, gamepad East | 8-tick hitstop on the turntable |
//! | `Esc` | Quit (desktop) |
//!
//! # Arguments
//!
//! - `--frames N`: exit after N frames, for unattended runs.
//! - `--msaa off|4`: override the MSAA setting.
//!
//! Desktop runs need cooked content: `cargo xtask run --platform macos` cooks and launches.

mod app;
mod input;
mod options;
mod orbit;
mod stats;
mod turntable;

use std::cell::RefCell;
use std::fmt::Write as _;
use std::sync::{Mutex, Once};

use mimalloc::MiMalloc;
use sapphire_core::alloc::CountingAllocator;
use sapphire_core::log::{LogConfig, LogLevel, LogRecord, LogSink};
use sapphire_crash::{CrashConfig, CrashGuard};
use sapphire_platform::{App, DeviceClass, Os};

pub use app::SandboxApp;

/// Every allocation is counted so the frame stats can show allocations per frame (rule R6).
#[global_allocator]
static GLOBAL: CountingAllocator<MiMalloc> = CountingAllocator::new(MiMalloc);

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
            // Reused per thread so logging doesn't allocate in steady state.
            static LINE: RefCell<String> = const { RefCell::new(String::new()) };
        }
        LINE.with_borrow_mut(|line| {
            line.clear();
            let _ = write!(line, "[{}] {}", record.target, record.message);
            sapphire_platform::sdl_log(tracing_level(record.level), line);
        });
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
