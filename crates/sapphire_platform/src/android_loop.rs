//! Android: the engine's vsync loop drives SDL's main callbacks, and a frame limiter starts frames
//! on the vsyncs the target rate is due on (spec §6.2, ADR 0004).
//!
//! SDL's `SDL_EnterAppMainCallbacks` on Android is its generic loop (`SDL_sysmain_callbacks.c`):
//! it runs `SDL_AppIterate` back to back, and FIFO presentation, waiting for a free swapchain image,
//! is all that slows it. So a tier built for 30 fps ran at the display's rate, about 60 fps on a
//! 60 Hz panel and up to 120 on a phone's. The platform runs that loop itself, with SDL's own
//! main-callback functions, and waits for a vsync from the NDK's `AChoreographer` before each
//! iteration. SDL then pumps and dispatches events and runs the app's iteration, where the frame
//! clock asks the pacer whether that vsync starts a frame and holds it back otherwise, as the
//! display link's ticks are on iOS (`ios_loop.rs`). Event dispatch, lifecycle events and quitting
//! stay SDL's.
//!
//! The choreographer runs on this thread's own looper: SDLActivity's SDL thread has none, so the
//! loop prepares one. Each wait posts one frame callback and sleeps in the looper until it reports
//! the next vsync, so a held tick sleeps to the next vsync rather than spinning, the thread sleeps
//! through vsyncs while a frame's work runs, and nothing crosses threads. On Android 11 (API 30)
//! and later the choreographer also reports the display's refresh period as it changes.
//!
//! The refresh-rate hint: every native window a [`SurfaceHandle`](crate::SurfaceHandle) holds is
//! asked for the target rate with `ANativeWindow_setFrameRate` (API 30, resolved at run time, since
//! the app's minimum is 29), so a 120 Hz panel can drop to a rate the target divides. Where the
//! panel can't switch, Android 12 and later may throttle the app's vsyncs to the rate instead (a
//! frame-rate override: the emulator's 60 Hz display gave a 30 fps tier 30 Hz vsyncs), and the
//! choreographer reports that rate as the refresh. Android hands the app a new native window after
//! every trip to the background, and the next surface handle taken from it is hinted in turn.
//!
//! SDL blocks in its event pump while the app is paused, inside the iteration whose vsync the loop
//! has already taken, so the frame clock drops that vsync when the app comes back
//! ([`discard_vsync`]) and the next one starts the frame.
//!
//! Everything here runs on the SDL thread: SDLActivity calls `SDL_main` there, surface handles are
//! taken and dropped there, and the choreographer's callbacks run inside its looper's polls.

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::mem;
use std::num::NonZeroU32;
use std::ptr::{self, NonNull};
use std::sync::OnceLock;
use std::time::Duration;

use sdl3_sys::init::{
    SDL_APP_CONTINUE, SDL_APP_FAILURE, SDL_AppEvent_func, SDL_AppInit_func, SDL_AppIterate_func,
    SDL_AppQuit_func, SDL_AppResult,
};
use sdl3_sys::version::{SDL_VERSION, SDL_VERSIONNUM};

use crate::LOG_TARGET;

// SDL's main-callback machinery (`src/main/SDL_main_callbacks.h`): not public API, but SDL is
// linked statically, so its definitions are part of this binary. These are the calls SDL's generic
// entry (`src/main/generic/SDL_sysmain_callbacks.c`, which Android uses) makes, and the ones
// `ios_loop.rs` makes for iOS. They aren't versioned like the public API, so any SDL upgrade,
// patch releases included, must re-check them and this file's copy of the generic entry against
// SDL's sources. The workspace pins `sdl3-sys` exactly, and the assertion below stops the build
// until someone has.
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
    "SDL changed: check the main-callback declarations and the loop in android_loop.rs against \
     SDL's src/main/SDL_main_callbacks.h and .c and src/main/generic/SDL_sysmain_callbacks.c, and \
     Android's pause handling in src/video/android/SDL_androidevents.c, then update this check \
     (ADR 0004)"
);

/// How long the loop waits for a vsync before it iterates anyway, on the clock. Longer than any
/// display's refresh (10 Hz and up), so it never cuts a vsync short; it keeps SDL's events and the
/// app going should the choreographer stop reporting, as with the display off.
const VSYNC_TIMEOUT: Duration = Duration::from_millis(100);

/// `ANATIVEWINDOW_FRAME_RATE_COMPATIBILITY_DEFAULT`: the app can run at whatever rate the system
/// picks (games and UI, as opposed to fixed-rate video).
const FRAME_RATE_COMPATIBILITY_DEFAULT: i8 = 0;

/// `ALooper_pollOnce`'s result when it failed.
const ALOOPER_POLL_ERROR: c_int = -4;

/// The NDK's `AChoreographer`, opaque.
#[repr(C)]
struct AChoreographer {
    _opaque: [u8; 0],
}

/// `AChoreographer_frameCallback64`: the vsync a posted frame callback ran for, in nanoseconds on
/// `CLOCK_MONOTONIC`, and the data pointer it was posted with.
type FrameCallback = unsafe extern "C" fn(frame_time_nanos: i64, data: *mut c_void);
/// `AChoreographer_refreshRateCallback`: the display's vsync period in nanoseconds.
type RefreshRateCallback = unsafe extern "C" fn(vsync_period_nanos: i64, data: *mut c_void);
/// `ANativeWindow_setFrameRate` (API 30).
type SetFrameRate = unsafe extern "C" fn(window: *mut c_void, frame_rate: f32, compatibility: i8) -> i32;
/// `AChoreographer_registerRefreshRateCallback` and `AChoreographer_unregisterRefreshRateCallback`
/// (API 30).
type RefreshRateRegistration = unsafe extern "C" fn(
    choreographer: *mut AChoreographer,
    callback: RefreshRateCallback,
    data: *mut c_void,
);

// In the NDK's API 29 stubs, the app's minimum.
#[link(name = "android")]
unsafe extern "C" {
    /// Gives the calling thread a looper, or returns the one it has.
    fn ALooper_prepare(opts: c_int) -> *mut c_void;
    /// Waits up to `timeout_millis` for the looper's events, running their callbacks.
    fn ALooper_pollOnce(
        timeout_millis: c_int,
        out_fd: *mut c_int,
        out_events: *mut c_int,
        out_data: *mut *mut c_void,
    ) -> c_int;
    /// The calling thread's choreographer; null unless the thread has a looper.
    fn AChoreographer_getInstance() -> *mut AChoreographer;
    /// Runs `callback` once, on the thread's looper, at the next vsync.
    fn AChoreographer_postFrameCallback64(
        choreographer: *mut AChoreographer,
        callback: FrameCallback,
        data: *mut c_void,
    );
}

thread_local! {
    /// The vsync the tick being iterated came on, in nanoseconds on `CLOCK_MONOTONIC`, for the frame
    /// clock to take.
    static VSYNC: Cell<Option<u64>> = const { Cell::new(None) };
    /// The vsync the last frame callback ran for, not yet taken by the loop.
    static REPORTED: Cell<Option<u64>> = const { Cell::new(None) };
    /// A frame callback is posted and hasn't run.
    static POSTED: Cell<bool> = const { Cell::new(false) };
    /// The display's refresh period, as the choreographer last reported it (API 30 and later).
    static REFRESH: Cell<Option<Duration>> = const { Cell::new(None) };
    /// The frame rate native windows are asked for; `None`: the system's choice.
    static RATE: Cell<Option<NonZeroU32>> = const { Cell::new(None) };
    /// The native windows that surface handles hold, one entry per handle.
    static WINDOWS: RefCell<Vec<NonNull<c_void>>> = const { RefCell::new(Vec::new()) };
}

/// `SDL_EnterAppMainCallbacks` for Android, waiting for a vsync before each iteration: initializes
/// the app through SDL, iterates until the app or SDL asks to stop, then shuts SDL down, as SDL's
/// generic entry does.
///
/// # Safety
///
/// Called once, from `SDL_main` on SDLActivity's SDL thread, with the `argc` / `argv` SDL passed it
/// and callbacks with SDL's signatures.
pub(crate) unsafe fn enter_main_callbacks(
    argc: c_int,
    argv: *mut *mut c_char,
    init: SDL_AppInit_func,
    iterate: SDL_AppIterate_func,
    event: SDL_AppEvent_func,
    quit: SDL_AppQuit_func,
) -> c_int {
    // SAFETY: forwarded caller contract; this is what SDL's generic entry calls.
    let mut result = unsafe { SDL_InitMainCallbacks(argc, argv, init, iterate, event, quit) };
    if result == SDL_APP_CONTINUE {
        let mut vsync = Vsync::new();
        tracing::info!(
            target: LOG_TARGET,
            refresh_hint = api30().set_frame_rate.is_some(),
            refresh_reports = api30().register_refresh_rate.is_some(),
            "vsync loop started",
        );
        while result == SDL_APP_CONTINUE {
            VSYNC.set(Some(vsync.wait()));
            // SAFETY: initialization succeeded and quitting hasn't started (the loop ends at the
            // first other result), so SDL's main callbacks are live; this is SDL's thread.
            result = unsafe { SDL_IterateMainCallbacks(true) };
            VSYNC.set(None);
        }
    }
    // SAFETY: called once, on SDL's thread, after SDL_InitMainCallbacks, as SDL's entry does.
    unsafe { SDL_QuitMainCallbacks(result) };
    if result == SDL_APP_FAILURE { 1 } else { 0 }
}

/// The vsync the tick being iterated came on, once: `None` outside the loop's tick, and after
/// [`discard_vsync`].
pub(crate) fn take_vsync() -> Option<u64> {
    VSYNC.take()
}

/// Drops the vsync of the tick being iterated: the app came back to the foreground during this
/// tick's event pump, which SDL blocks in while the app is paused, so the vsync is from before the
/// trip. The loop's next vsync starts the next frame.
pub(crate) fn discard_vsync() {
    VSYNC.set(None);
}

/// The display's refresh period as the choreographer last reported it; `None` before Android 11
/// (API 30) or until its first report.
pub(crate) fn refresh_period() -> Option<Duration> {
    REFRESH.get()
}

/// Asks every native window a surface handle holds for `rate` (`None`: the system's choice), and
/// the windows of surface handles taken later.
pub(crate) fn set_frame_rate(rate: Option<NonZeroU32>) {
    if RATE.replace(rate) == rate {
        return;
    }
    WINDOWS.with_borrow(|windows| {
        for (index, &window) in windows.iter().enumerate() {
            // A window two surface handles hold is hinted once.
            if !windows[..index].contains(&window) {
                hint(window, rate);
            }
        }
    });
}

/// A surface handle took a reference to `window`: hints it at the current rate, if any (a new
/// window has the system's choice already), and keeps it for later changes until
/// [`window_released`].
pub(crate) fn window_acquired(window: NonNull<c_void>) {
    let hinted = WINDOWS.with_borrow_mut(|windows| {
        let hinted = windows.contains(&window);
        windows.push(window);
        hinted
    });
    if let Some(rate) = RATE.get()
        && !hinted
    {
        hint(window, Some(rate));
    }
}

/// The surface handle's reference to `window` is about to be released.
///
/// Runs in the handle's `Drop`, which may come during this thread's teardown, after the list is
/// gone (a handle kept in another thread-local, say): there is nothing to remove from then, and
/// panicking in a destructor would abort (review PA-005).
pub(crate) fn window_released(window: NonNull<c_void>) {
    let _ = WINDOWS.try_with(|windows| {
        let mut windows = windows.borrow_mut();
        if let Some(index) = windows.iter().position(|&held| held == window) {
            windows.swap_remove(index);
        }
    });
}

/// Asks the display, through `window`, for `rate`; nothing before Android 11 (API 30).
fn hint(window: NonNull<c_void>, rate: Option<NonZeroU32>) {
    let Some(set_frame_rate) = api30().set_frame_rate else { return };
    // Exact for every rate a tier can ask for (at most 240).
    let frame_rate = rate.map_or(0.0, |rate| rate.get() as f32);
    // SAFETY: a surface handle holds a reference to the window, so it is alive; 0 fps is the
    // system's choice, and the compatibility is one the NDK defines.
    let status = unsafe { set_frame_rate(window.as_ptr(), frame_rate, FRAME_RATE_COMPATIBILITY_DEFAULT) };
    if status == 0 {
        tracing::info!(target: LOG_TARGET, ?rate, "refresh rate hinted");
    } else {
        tracing::warn!(target: LOG_TARGET, ?rate, status, "ANativeWindow_setFrameRate failed");
    }
}

/// The NDK functions newer than the app's minimum (API 29), where the device has them.
struct Api30 {
    set_frame_rate: Option<SetFrameRate>,
    register_refresh_rate: Option<RefreshRateRegistration>,
    unregister_refresh_rate: Option<RefreshRateRegistration>,
}

fn api30() -> &'static Api30 {
    static API: OnceLock<Api30> = OnceLock::new();
    API.get_or_init(|| {
        // SAFETY: libandroid.so is a system library the app already links against (dlopen only
        // takes another reference to it), and the handle is never closed.
        let library = unsafe { libc::dlopen(c"libandroid.so".as_ptr(), libc::RTLD_NOW) };
        let symbol = |name: &CStr| {
            if library.is_null() {
                return None;
            }
            // SAFETY: `library` is a live handle and `name` a C string.
            NonNull::new(unsafe { libc::dlsym(library, name.as_ptr()) })
        };
        let set_frame_rate = symbol(c"ANativeWindow_setFrameRate");
        let register = symbol(c"AChoreographer_registerRefreshRateCallback");
        let unregister = symbol(c"AChoreographer_unregisterRefreshRateCallback");
        // SAFETY: a function symbol's address is the function, and the NDK declares these three
        // with exactly these signatures (`native_window.h`, `choreographer.h`).
        unsafe {
            Api30 {
                set_frame_rate: set_frame_rate
                    .map(|f| mem::transmute::<*mut c_void, SetFrameRate>(f.as_ptr())),
                register_refresh_rate: register
                    .map(|f| mem::transmute::<*mut c_void, RefreshRateRegistration>(f.as_ptr())),
                unregister_refresh_rate: unregister
                    .map(|f| mem::transmute::<*mut c_void, RefreshRateRegistration>(f.as_ptr())),
            }
        }
    })
}

/// The vsyncs of this thread's choreographer.
struct Vsync {
    choreographer: NonNull<AChoreographer>,
    /// The last tick handed out, in nanoseconds on `CLOCK_MONOTONIC`.
    last: Option<u64>,
}

impl Vsync {
    /// Gives this thread a looper and takes its choreographer, registering for its refresh-rate
    /// reports where the device has them.
    fn new() -> Self {
        // SAFETY: no preconditions; 0 asks for a looper whose events all have callbacks.
        unsafe { ALooper_prepare(0) };
        // SAFETY: the thread has a looper now.
        let choreographer = NonNull::new(unsafe { AChoreographer_getInstance() })
            .expect("a thread with a looper has a choreographer");
        if let Some(register) = api30().register_refresh_rate {
            // SAFETY: the choreographer is this thread's, and `on_refresh_rate` matches the
            // callback type and ignores its data. It runs only inside this thread's polls.
            unsafe { register(choreographer.as_ptr(), on_refresh_rate, ptr::null_mut()) };
        }
        Self { choreographer, last: None }
    }

    /// Waits for the next vsync and returns its time, or after [`VSYNC_TIMEOUT`] without one, the
    /// time then. Each tick is later than the one before.
    fn wait(&mut self) -> u64 {
        let deadline = monotonic_now() + duration_nanos(VSYNC_TIMEOUT);
        loop {
            if let Some(vsync) = REPORTED.take()
                // A vsync no later than the last tick comes from a callback posted before a
                // timeout: the frame it was for has started.
                && self.last.is_none_or(|last| vsync > last)
            {
                self.last = Some(vsync);
                return vsync;
            }
            if !POSTED.replace(true) {
                // SAFETY: the choreographer is this thread's, and `on_vsync` matches the callback
                // type and ignores its data. It runs only inside this thread's polls.
                unsafe {
                    AChoreographer_postFrameCallback64(self.choreographer.as_ptr(), on_vsync, ptr::null_mut())
                };
            }
            let now = monotonic_now();
            if now >= deadline {
                // Iterate on the clock; the callback stays posted for the next wait.
                let tick = self.last.map_or(now, |last| now.max(last + 1));
                self.last = Some(tick);
                return tick;
            }
            let timeout = c_int::try_from((deadline - now).div_ceil(1_000_000)).unwrap_or(c_int::MAX);
            // SAFETY: the thread has a looper (`new`), and no out-parameters are asked for.
            if unsafe { ALooper_pollOnce(timeout, ptr::null_mut(), ptr::null_mut(), ptr::null_mut()) }
                == ALOOPER_POLL_ERROR
            {
                // Not expected (the looper retries interrupted waits itself); sleep out the wait
                // rather than spin on it.
                tracing::warn!(target: LOG_TARGET, "ALooper_pollOnce failed; waiting out the vsync timeout");
                std::thread::sleep(Duration::from_nanos(deadline.saturating_sub(monotonic_now())));
            }
        }
    }
}

impl Drop for Vsync {
    fn drop(&mut self) {
        if let Some(unregister) = api30().unregister_refresh_rate {
            // SAFETY: as registered in `new`, on this thread.
            unsafe { unregister(self.choreographer.as_ptr(), on_refresh_rate, ptr::null_mut()) };
        }
    }
}

/// The choreographer's frame callback: records the vsync for [`Vsync::wait`].
unsafe extern "C" fn on_vsync(frame_time_nanos: i64, _data: *mut c_void) {
    // Runs inside the loop's poll, on its thread, whose thread-locals are alive; `try_with` keeps
    // even a call during the thread's teardown from unwinding into the NDK.
    let _ = POSTED.try_with(|posted| posted.set(false));
    let _ = REPORTED.try_with(|reported| reported.set(u64::try_from(frame_time_nanos).ok()));
}

/// The choreographer's refresh-rate callback (API 30): the display's vsync period, at
/// registration (Android 12 and later report it then) and whenever it changes.
unsafe extern "C" fn on_refresh_rate(vsync_period_nanos: i64, _data: *mut c_void) {
    let period = u64::try_from(vsync_period_nanos).ok().filter(|&nanos| nanos > 0).map(Duration::from_nanos);
    // As in `on_vsync`.
    let _ = REFRESH.try_with(|refresh| refresh.set(period));
}

/// Now on `CLOCK_MONOTONIC`, the choreographer's clock, in nanoseconds.
fn monotonic_now() -> u64 {
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `now` is a valid out-parameter, and every Android kernel has CLOCK_MONOTONIC.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    let seconds = u64::try_from(now.tv_sec).unwrap_or(0);
    let nanos = u64::try_from(now.tv_nsec).unwrap_or(0);
    seconds * 1_000_000_000 + nanos
}

fn duration_nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}
