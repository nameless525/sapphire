//! End-to-end checks of `run` on the real SDL main-callback loop: exit codes, callback
//! order and panic propagation.
//!
//! SDL requires its video calls on the process's main thread, so this test supplies its
//! own `main` (`harness = false`). It runs SDL's headless "dummy" video driver, so it
//! needs no display or GPU.

use std::cell::RefCell;
use std::panic::{self, AssertUnwindSafe};
use std::process::ExitCode;

use sapphire_platform::{App, AppControl, AppInitError, AppMetadata, Event, LifecycleEvent, Platform, run};
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

fn main() {
    // SAFETY: nothing else runs yet; this is the only thread.
    unsafe { std::env::set_var("SDL_VIDEO_DRIVER", "dummy") };

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
