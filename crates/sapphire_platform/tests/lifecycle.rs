//! End-to-end checks of `run` on the real SDL main-callback loop: exit codes, callback
//! order and panic propagation.
//!
//! SDL requires its video calls on the process's main thread, so this test supplies its
//! own `main` (`harness = false`). It runs SDL's headless "dummy" video driver, so it
//! needs no display or GPU.

use std::cell::RefCell;
use std::num::NonZeroU32;
use std::panic::{self, AssertUnwindSafe};
use std::process::ExitCode;

use sapphire_platform::raw_window_handle::{HandleError, HasDisplayHandle, HasWindowHandle};
use sapphire_platform::{
    App, AppControl, AppInitError, AppMetadata, Event, LifecycleEvent, PacingSource, Platform, Size,
    SurfaceHandle, Window, WindowDesc, run,
};
use sdl3_sys::events::{
    SDL_EVENT_DID_ENTER_FOREGROUND, SDL_EVENT_LOW_MEMORY, SDL_EVENT_WILL_ENTER_BACKGROUND, SDL_Event,
    SDL_PushEvent,
};

thread_local! {
    /// Callback trace of the current run; callbacks all run on this thread.
    static TRACE: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

fn record(step: &'static str) {
    TRACE.with_borrow_mut(|trace| trace.push(step));
}

fn take_trace() -> Vec<&'static str> {
    TRACE.with_borrow_mut(std::mem::take)
}

const METADATA: AppMetadata = AppMetadata {
    name: "Lifecycle Test",
    organization: "Sapphire",
    identifier: "dev.sapphire.lifecycle_test",
    version: "0.0.0",
};

/// Exits cleanly after three frames.
struct ThreeFrames(u32);

impl App for ThreeFrames {
    const METADATA: AppMetadata = METADATA;

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        record("init");
        assert!(platform.user_dir().is_dir(), "user dir must exist");
        assert!(!platform.args().is_empty(), "args include the program name");
        Ok(Self(0))
    }
    fn event(&mut self, _: &mut Platform, _: &Event) -> AppControl {
        AppControl::Continue
    }
    fn iterate(&mut self, _: &mut Platform) -> AppControl {
        record("iterate");
        self.0 += 1;
        if self.0 == 3 { AppControl::Exit } else { AppControl::Continue }
    }
    fn quit(&mut self, _: &mut Platform) {
        record("quit");
    }
}

impl Drop for ThreeFrames {
    fn drop(&mut self) {
        record("drop");
    }
}

/// Fails to initialize.
struct FailsInit;

impl App for FailsInit {
    const METADATA: AppMetadata = METADATA;

    fn init(_: &mut Platform) -> Result<Self, AppInitError> {
        record("init");
        Err(AppInitError::msg("deliberate init failure"))
    }
    fn event(&mut self, _: &mut Platform, _: &Event) -> AppControl {
        unreachable!("never initialized")
    }
    fn iterate(&mut self, _: &mut Platform) -> AppControl {
        unreachable!("never initialized")
    }
}

/// Panics on its second frame.
struct PanicsInIterate(u32);

impl App for PanicsInIterate {
    const METADATA: AppMetadata = METADATA;

    fn init(_: &mut Platform) -> Result<Self, AppInitError> {
        Ok(Self(0))
    }
    fn event(&mut self, _: &mut Platform, _: &Event) -> AppControl {
        AppControl::Continue
    }
    fn iterate(&mut self, _: &mut Platform) -> AppControl {
        self.0 += 1;
        if self.0 == 2 {
            panic!("deliberate panic in iterate");
        }
        AppControl::Continue
    }
    fn quit(&mut self, _: &mut Platform) {
        record("quit");
    }
}

impl Drop for PanicsInIterate {
    fn drop(&mut self) {
        record("drop");
    }
}

/// Raises lifecycle events from inside `iterate`. SDL dispatches lifecycle events
/// synchronously, so each one re-enters the event callback while `iterate` is running.
struct ReentrantLifecycle(u32);

impl App for ReentrantLifecycle {
    const METADATA: AppMetadata = METADATA;

    fn init(_: &mut Platform) -> Result<Self, AppInitError> {
        Ok(Self(0))
    }
    fn event(&mut self, _: &mut Platform, event: &Event) -> AppControl {
        if let Event::Lifecycle { timestamp, event } = event {
            assert!(timestamp.as_nanos() > 0, "lifecycle events carry a timestamp");
            record(match event {
                LifecycleEvent::LowMemory => "low memory",
                LifecycleEvent::DidEnterForeground => "did enter foreground",
                LifecycleEvent::WillEnterBackground => "will enter background",
                _ => "other lifecycle event",
            });
        }
        AppControl::Continue
    }
    fn iterate(&mut self, _: &mut Platform) -> AppControl {
        self.0 += 1;
        if self.0 == 2 {
            return AppControl::Exit;
        }
        record("iterate start");
        for kind in [
            SDL_EVENT_LOW_MEMORY,
            SDL_EVENT_DID_ENTER_FOREGROUND,
            SDL_EVENT_DID_ENTER_FOREGROUND,
            SDL_EVENT_WILL_ENTER_BACKGROUND,
        ] {
            let mut event = SDL_Event::default();
            event.r#type = kind.0;
            // SAFETY: SDL is initialized and `event` is a valid, fully initialized event.
            assert!(unsafe { SDL_PushEvent(&mut event) });
        }
        record("iterate end");
        AppControl::Continue
    }
}

/// Sets a frame rate, then checks on its fifth frame that SDL's loop ran every frame: on desktop
/// presentation paces frames, and the target is only measured against (M1 F7).
struct PacedByPresentation(u32);

impl App for PacedByPresentation {
    const METADATA: AppMetadata = METADATA;

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        platform.set_frame_rate(NonZeroU32::new(60));
        Ok(Self(0))
    }
    fn event(&mut self, _: &mut Platform, _: &Event) -> AppControl {
        AppControl::Continue
    }
    fn iterate(&mut self, platform: &mut Platform) -> AppControl {
        self.0 += 1;
        if self.0 < 5 {
            return AppControl::Continue;
        }
        let pacing = platform.frame_pacing();
        assert_eq!(pacing.source, PacingSource::Presentation);
        assert_eq!(pacing.target, NonZeroU32::new(60));
        assert_eq!((pacing.stats.frames, pacing.stats.held), (5, 0));
        assert_eq!(platform.frame_delta(), None, "only a display link's intervals are vsync-exact");
        record("paced");
        AppControl::Exit
    }
}

thread_local! {
    /// What [`KeepsWindow`] keeps past its run (review PA-005): its window, and a surface handle
    /// where the video driver gives one.
    static KEPT: RefCell<Option<(Window, Result<SurfaceHandle, HandleError>)>> = const { RefCell::new(None) };
}

/// Creates a window, takes a surface handle from it and keeps both in a thread-local past the
/// end of its run, as safe library code can. Exits after one frame.
struct KeepsWindow;

impl App for KeepsWindow {
    const METADATA: AppMetadata = METADATA;

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        match platform.create_window(&WindowDesc::default()) {
            Ok(window) => {
                let handle = window.surface_handle();
                if let Ok(handle) = &handle {
                    assert!(handle.window_handle().is_ok(), "a live handle borrows");
                }
                record(if handle.is_ok() { "kept window and handle" } else { "kept window" });
                KEPT.with_borrow_mut(|kept| *kept = Some((window, handle)));
            }
            // The dummy driver has no Metal, which every Apple window is created with.
            Err(_) => record("no window"),
        }
        Ok(Self)
    }
    fn event(&mut self, _: &mut Platform, _: &Event) -> AppControl {
        AppControl::Continue
    }
    fn iterate(&mut self, _: &mut Platform) -> AppControl {
        AppControl::Exit
    }
}

/// Checks during a later run, once SDL has started again, that what [`KeepsWindow`] kept stays
/// stale, then drops it: SDL destroyed it with the earlier run, so dropping it must not touch SDL.
struct ChecksKeptWindow;

impl App for ChecksKeptWindow {
    const METADATA: AppMetadata = METADATA;

    fn init(_: &mut Platform) -> Result<Self, AppInitError> {
        Ok(Self)
    }
    fn event(&mut self, _: &mut Platform, _: &Event) -> AppControl {
        AppControl::Continue
    }
    fn iterate(&mut self, _: &mut Platform) -> AppControl {
        check_kept_window("during a later run");
        KEPT.with_borrow_mut(Option::take);
        AppControl::Exit
    }
}

/// What [`KeepsWindow`] kept reports itself unavailable rather than lending SDL objects that are
/// gone.
fn check_kept_window(when: &str) {
    KEPT.with_borrow(|kept| {
        let Some((window, handle)) = kept else { return };
        assert!(
            matches!(window.surface_handle(), Err(HandleError::Unavailable)),
            "{when}: a window from an ended run gives no surface handle"
        );
        assert_eq!(window.size(), Size::default(), "{when}");
        assert_eq!(window.pixel_size(), Size::default(), "{when}");
        if let Ok(handle) = handle {
            assert!(matches!(handle.window_handle(), Err(HandleError::Unavailable)), "{when}");
            assert!(matches!(handle.display_handle(), Err(HandleError::Unavailable)), "{when}");
            assert_eq!(handle.metal_layer(), None, "{when}");
        }
    });
}

fn main() {
    // SAFETY: nothing else runs yet; this is the only thread.
    unsafe { std::env::set_var("SDL_VIDEO_DRIVER", "dummy") };

    // Review PA-005: a window and surface handle kept past `run` are stale, also once a later
    // run has started SDL again.
    assert_eq!(run::<KeepsWindow>(), ExitCode::SUCCESS);
    let kept = take_trace();
    eprintln!("lifecycle: KeepsWindow {kept:?}");
    // The dummy driver makes windows everywhere but on Apple targets, whose windows all have
    // Metal views (the platform crate's unit tests cover those with a made-up window).
    if !cfg!(target_vendor = "apple") {
        assert!(kept.iter().any(|step| step.starts_with("kept window")), "{kept:?}");
    }
    check_kept_window("after its run");
    assert_eq!(run::<ChecksKeptWindow>(), ExitCode::SUCCESS);

    assert_eq!(run::<ThreeFrames>(), ExitCode::SUCCESS);
    assert_eq!(take_trace(), ["init", "iterate", "iterate", "iterate", "quit", "drop"]);

    // Re-entrant events are held back until `iterate` returns, then delivered in order,
    // with the repeated foreground transition dropped.
    assert_eq!(run::<ReentrantLifecycle>(), ExitCode::SUCCESS);
    assert_eq!(
        take_trace(),
        ["iterate start", "iterate end", "low memory", "did enter foreground", "will enter background"]
    );

    assert_eq!(run::<FailsInit>(), ExitCode::FAILURE);
    assert_eq!(take_trace(), ["init"]);

    assert_eq!(run::<PacedByPresentation>(), ExitCode::SUCCESS);
    assert_eq!(take_trace(), ["paced"]);

    // Keep the expected panic's message out of the test output.
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let result = panic::catch_unwind(AssertUnwindSafe(run::<PanicsInIterate>));
    panic::set_hook(default_hook);
    let payload = result.expect_err("the panic must resume out of `run`");
    assert_eq!(payload.downcast_ref::<&str>(), Some(&"deliberate panic in iterate"));
    // After a panic the app is dropped (releasing its windows) but `quit` is skipped.
    assert_eq!(take_trace(), ["drop"]);

    eprintln!("lifecycle: ok");
}
