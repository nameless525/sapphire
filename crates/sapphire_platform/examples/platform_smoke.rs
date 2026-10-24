//! Platform smoke test (spec §5.1): opens a window, logs every event at debug level and
//! exits when the window is closed or, with `--frames N`, after N frames.
//!
//! ```text
//! cargo run -p sapphire_platform --example platform_smoke -- --frames 120
//! ```
//!
//! Set `RUST_LOG` to change the log filter (default `debug`). Pass arguments on iOS with
//! `xcrun simctl launch <device> <bundle id> --frames 120`, and on Android through
//! `SDLActivity.getArguments()`. The Android build is the `platform_smoke_android`
//! example, which wraps this file in the `cdylib` SDLActivity loads.

use std::process::ExitCode;
use std::time::Duration;

use sapphire_platform::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use sapphire_platform::{
    App, AppControl, AppInitError, AppMetadata, Event, Platform, Window, WindowDesc, WindowEvent, sdl_log,
};
use tracing::{debug, info};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const LOG_TARGET: &str = "sapphire::platform_smoke";

pub(crate) struct Smoke {
    window: Window,
    frames: u64,
    max_frames: Option<u64>,
}

impl App for Smoke {
    const METADATA: AppMetadata = AppMetadata {
        name: "Platform Smoke",
        organization: "Sapphire",
        identifier: "dev.sapphire.platform_smoke",
        version: env!("CARGO_PKG_VERSION"),
    };

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let max_frames = parse_frames(platform.args())?;
        let window = platform.create_window(&WindowDesc {
            title: String::from("Sapphire platform smoke"),
            ..WindowDesc::default()
        })?;

        // What a renderer would build its surface from; nothing renders here.
        let handle = window.surface_handle()?;
        let surface = match handle.metal_layer() {
            Some(layer) => format!("CAMetalLayer at {layer:p}"),
            None => format!(
                "{:?} / {:?}",
                handle.window_handle().map(|h| h.as_raw()),
                handle.display_handle().map(|h| h.as_raw()),
            ),
        };
        info!(
            target: LOG_TARGET,
            window = ?window.id(),
            size = ?window.size(),
            pixel_size = ?window.pixel_size(),
            display_scale = window.display_scale(),
            safe_area = ?window.safe_area(),
            surface,
            content_root = platform.content().root(),
            user_dir = %platform.user_dir().display(),
            cache_dir = %platform.cache_dir().display(),
            system = ?platform.system_info(),
            ?max_frames,
            "window created",
        );
        Ok(Self { window, frames: 0, max_frames })
    }

    fn event(&mut self, platform: &mut Platform, event: &Event) -> AppControl {
        let age = platform.now().saturating_duration_since(event.timestamp());
        debug!(target: LOG_TARGET, ?age, ?event);
        match event {
            Event::Quit { .. } => AppControl::Exit,
            Event::Window { window, event: WindowEvent::CloseRequested, .. }
                if *window == self.window.id() =>
            {
                AppControl::Exit
            }
            _ => AppControl::Continue,
        }
    }

    fn iterate(&mut self, _platform: &mut Platform) -> AppControl {
        self.frames += 1;
        if self.max_frames.is_some_and(|max| self.frames >= max) {
            info!(target: LOG_TARGET, frames = self.frames, "frame limit reached");
            return AppControl::Exit;
        }
        // Nothing presents yet, so nothing waits for vsync: approximate a 60 Hz frame.
        std::thread::sleep(Duration::from_millis(16));
        AppControl::Continue
    }

    fn quit(&mut self, _platform: &mut Platform) {
        info!(target: LOG_TARGET, frames = self.frames, "quitting");
    }
}

fn parse_frames(args: &[String]) -> Result<Option<u64>, AppInitError> {
    let mut args = args.iter().skip(1);
    let mut frames = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--frames" => {
                let value = args.next().ok_or_else(|| AppInitError::msg("--frames needs a value"))?;
                let count =
                    value.parse().map_err(|e| AppInitError::msg(format!("--frames {value:?}: {e}")))?;
                frames = Some(count);
            }
            other => return Err(AppInitError::msg(format!("unknown argument {other:?}"))),
        }
    }
    Ok(frames)
}

/// Logs to stderr on desktop and through `SDL_Log` (logcat / unified log) on mobile.
pub(crate) fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug"));
    let registry = tracing_subscriber::registry().with(filter);
    if cfg!(any(target_os = "ios", target_os = "android")) {
        registry.with(SdlLogLayer).init();
    } else {
        registry.with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr)).init();
    }
}

/// Minimal `tracing` layer forwarding formatted events to [`sdl_log`].
struct SdlLogLayer;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for SdlLogLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        struct Fields(String);
        impl tracing::field::Visit for Fields {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                use std::fmt::Write;
                let separator = if self.0.is_empty() { "" } else { " " };
                let _ = if field.name() == "message" {
                    write!(self.0, "{separator}{value:?}")
                } else {
                    write!(self.0, "{separator}{}={value:?}", field.name())
                };
            }
        }
        let mut fields = Fields(String::new());
        event.record(&mut fields);
        let metadata = event.metadata();
        sdl_log(*metadata.level(), &format!("{}: {}", metadata.target(), fields.0));
    }
}

// Unused inside `platform_smoke_android`, where `SDL_main` is the entry point.
#[cfg_attr(target_os = "android", allow(dead_code))]
fn main() -> ExitCode {
    init_logging();
    sapphire_platform::run::<Smoke>()
}
