//! iOS: the engine's display link drives SDL's main callbacks (spec §6.2, ADR 0003).
//!
//! SDL's `SDL_EnterAppMainCallbacks` for iOS runs `SDL_AppIterate` from a `CADisplayLink` of its
//! own, at the display's full rate (up to 120 Hz on ProMotion), ignoring
//! `SDL_HINT_MAIN_CALLBACK_RATE`; and each SDL window's view controller keeps another link asking
//! for 80–120 Hz. Neither can be reached through SDL's API, and either keeps a ProMotion panel at
//! 120 Hz. So the platform does what SDL's iOS entry does (its `SDL_sysmain_callbacks.m`) with a
//! link of its own: SDL's own main-callback functions set up, iterate and quit (event dispatch,
//! lifecycle events, quitting all stay SDL's), and the engine's link, at the target frame rate,
//! calls the iteration. The windows' links are stopped as each window is created.
//!
//! Everything here runs on the main thread: UIKit calls `SDL_main` there, and the link is added
//! to the main run loop.

use std::cell::{Cell, RefCell};
use std::ffi::{c_char, c_int};
use std::num::NonZeroU32;
use std::panic::{self, AssertUnwindSafe};
use std::ptr::{self, NonNull};

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::{NSDefaultRunLoopMode, NSRunLoop};
use objc2_quartz_core::{CADisplayLink, CAFrameRateRange, CAFrameRateRangeDefault};
use sdl3_sys::init::{
    SDL_APP_CONTINUE, SDL_APP_FAILURE, SDL_AppEvent_func, SDL_AppInit_func, SDL_AppIterate_func,
    SDL_AppQuit_func, SDL_AppResult,
};
use sdl3_sys::system::SDL_SetiOSAnimationCallback;
use sdl3_sys::version::{SDL_VERSION, SDL_VERSIONNUM};
use sdl3_sys::video::SDL_Window;

use crate::{LOG_TARGET, sdl};

// SDL's main-callback machinery (`src/main/SDL_main_callbacks.h`): not public API, but SDL is
// linked statically, so its definitions are part of this binary. These are the calls SDL's own
// iOS entry makes. They aren't versioned like the public API, so any SDL upgrade, patch releases
// included, must re-check them and this file's copy of SDL's iOS entry against SDL's sources. The
// workspace pins `sdl3-sys` exactly, and the assertion below stops the build until someone has.
unsafe extern "C" {
    /// Stores the callbacks, runs `appinit`, then installs SDL's event watch, which delivers
    /// lifecycle events (backgrounding, memory warnings, termination) the moment they happen.
    fn SDL_InitMainCallbacks(
        argc: c_int,
        argv: *mut *mut c_char,
        appinit: SDL_AppInit_func,
        appiter: SDL_AppIterate_func,
        appevent: SDL_AppEvent_func,
        appquit: SDL_AppQuit_func,
    ) -> SDL_AppResult;
    /// Pumps and dispatches queued events, then runs `appiter` unless the app is quitting.
    fn SDL_IterateMainCallbacks(pump_events: bool) -> SDL_AppResult;
    /// Removes the event watch, runs `appquit` and shuts SDL down.
    fn SDL_QuitMainCallbacks(result: SDL_AppResult);
}

const _: () = assert!(
    SDL_VERSION == SDL_VERSIONNUM(3, 4, 18),
    "SDL changed: check the main-callback declarations and the entry in ios_loop.rs against SDL's \
     src/main/SDL_main_callbacks.h and .c and src/main/ios/SDL_sysmain_callbacks.m, then update this \
     check (ADR 0003)"
);

thread_local! {
    /// The engine's display link, once SDL's initialization has succeeded.
    static LINK: RefCell<Option<Retained<CADisplayLink>>> = const { RefCell::new(None) };
    /// The frame rate the link asks the display for; `None`: the display's own.
    static RATE: Cell<Option<NonZeroU32>> = const { Cell::new(None) };
    /// The vsync the tick being iterated will be shown at, in nanoseconds, for the frame clock to
    /// take.
    static VSYNC: Cell<Option<u64>> = const { Cell::new(None) };
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this class doesn't implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SapphireFrameLinkTarget"]
    /// The object the display link calls on every tick.
    struct FrameLinkTarget;

    impl FrameLinkTarget {
        // SAFETY: CADisplayLink calls its selector with itself as the only argument.
        #[unsafe(method(frame:))]
        fn frame(&self, link: &CADisplayLink) {
            on_tick(link);
        }
    }

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for FrameLinkTarget {}
);

impl FrameLinkTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's `init` takes no arguments and returns the initialized object.
        unsafe { msg_send![super(this), init] }
    }
}

/// `SDL_EnterAppMainCallbacks` for iOS, with the engine's display link: initializes the app
/// through SDL, then returns, leaving UIKit's run loop to fire the link. If initialization asked
/// to stop, SDL shuts down and the process exits.
///
/// # Safety
///
/// Called once, from `SDL_main` on the main thread, with the `argc` / `argv` SDL passed it and
/// callbacks with SDL's signatures.
pub(crate) unsafe fn enter_main_callbacks(
    argc: c_int,
    argv: *mut *mut c_char,
    init: SDL_AppInit_func,
    iterate: SDL_AppIterate_func,
    event: SDL_AppEvent_func,
    quit: SDL_AppQuit_func,
) -> c_int {
    // SAFETY: forwarded caller contract; this is what SDL's own iOS entry calls.
    let result = unsafe { SDL_InitMainCallbacks(argc, argv, init, iterate, event, quit) };
    if result != SDL_APP_CONTINUE {
        finish(result);
    }
    let mtm = MainThreadMarker::new().expect("UIKit calls SDL_main on the main thread");
    let target = FrameLinkTarget::new(mtm);
    // SAFETY: `target` implements `frame:`, taking the link, and the link keeps it alive.
    let link = unsafe { CADisplayLink::displayLinkWithTarget_selector(&target, sel!(frame:)) };
    apply_rate(&link, RATE.get());
    // SAFETY: this is the main thread, whose run loop UIKit runs; the default mode is the one
    // SDL's link used, so frames pause in the same modal states as before.
    unsafe { link.addToRunLoop_forMode(&NSRunLoop::currentRunLoop(), NSDefaultRunLoopMode) };
    LINK.set(Some(link));
    tracing::info!(target: LOG_TARGET, rate = ?RATE.get(), "display link started");
    // Like SDL's entry: SDL_main returns into UIKit, whose run loop now fires the link.
    0
}

/// One display tick: SDL pumps and dispatches events and runs the app's iteration, which the
/// frame clock may turn down (the tick came sooner than the target interval).
fn on_tick(link: &CADisplayLink) {
    // Nothing may unwind into Core Animation: the app's callbacks already catch their panics, and
    // a panic here (in the platform itself) ends the process as a crash.
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        // The vsync this frame will be shown at, not the one just shown (`timestamp`): intervals
        // between frames' target times are the time each frame will be on screen, including the
        // first frame after a change of rate, as Apple advises for timing content.
        VSYNC.set(Some(seconds_to_nanos(link.targetTimestamp())));
        // SAFETY: initialization succeeded and quitting hasn't started (the link is invalidated
        // first), so SDL's main callbacks are live; this is the main thread.
        let result = unsafe { SDL_IterateMainCallbacks(true) };
        VSYNC.set(None);
        result
    }));
    match result {
        Ok(result) if result == SDL_APP_CONTINUE => {}
        Ok(result) => {
            link.invalidate();
            finish(result);
        }
        Err(_) => {
            tracing::error!(target: LOG_TARGET, "the display link's tick panicked (reported above); aborting");
            std::process::abort();
        }
    }
}

/// Shuts SDL down, running the app's quit callback, and ends the process as SDL's iOS entry does.
fn finish(result: SDL_AppResult) -> ! {
    // SAFETY: called once, on the main thread, after SDL_InitMainCallbacks.
    unsafe { SDL_QuitMainCallbacks(result) };
    std::process::exit(if result == SDL_APP_FAILURE { 1 } else { 0 })
}

/// Asks the display for `rate` from the next tick on (`None`: its own rate).
pub(crate) fn set_frame_rate(rate: Option<NonZeroU32>) {
    if RATE.replace(rate) == rate {
        return;
    }
    LINK.with_borrow(|link| {
        if let Some(link) = link {
            apply_rate(link, rate);
            tracing::info!(target: LOG_TARGET, ?rate, "display link rate changed");
        }
    });
}

/// The vsync the tick being iterated will be shown at, once: `None` outside the display link's
/// tick.
pub(crate) fn take_vsync() -> Option<u64> {
    VSYNC.take()
}

/// Stops the display link SDL's view controller starts for each window on a display faster than
/// 60 Hz: it does nothing on its ticks, but asks for 80–120 Hz, which would hold a ProMotion panel
/// at 120 Hz whatever the engine's link asks for.
pub(crate) fn stop_window_link(window: NonNull<SDL_Window>) {
    // SAFETY: the window is live and this is the main thread. With no callback, SDL only stops
    // the view controller's link and starts none.
    if !unsafe { SDL_SetiOSAnimationCallback(window.as_ptr(), 1, None, ptr::null_mut()) } {
        tracing::warn!(target: LOG_TARGET, error = %sdl::last_error(), "can't stop SDL's window display link");
    }
}

/// An exact rate as a range of one rate, so the link ticks on the vsyncs of that rate alone.
fn apply_rate(link: &CADisplayLink, rate: Option<NonZeroU32>) {
    let range = match rate {
        Some(rate) => {
            let hz = rate.get() as f32;
            CAFrameRateRange::new(hz, hz, hz)
        }
        // SAFETY: a constant Core Animation exports.
        None => unsafe { CAFrameRateRangeDefault },
    };
    link.setPreferredFrameRateRange(range);
}

/// Core Animation's time (`CACurrentMediaTime`, seconds of host time) in nanoseconds.
fn seconds_to_nanos(seconds: f64) -> u64 {
    (seconds * 1e9) as u64
}
