//! App lifecycle on SDL's main callbacks (spec §4.2, §5.1).
//!
//! SDL owns the main loop on every platform: it calls `SDL_AppInit` once,
//! `SDL_AppEvent` per event, `SDL_AppIterate` once per frame and `SDL_AppQuit` once at
//! the end. The `extern "C"` trampolines here adapt those calls to the [`App`] trait. On iOS
//! the engine's display link, not SDL's, starts each iteration (`ios_loop.rs`, ADR 0003), on
//! Android the engine's vsync loop (`android_loop.rs`, ADR 0004), and everywhere the platform's
//! frame clock may turn an iteration down before the app sees it.
//!
//! Panics never unwind into SDL's C frames. Each callback catches them, stashes the
//! first payload and reports failure, so SDL winds down and calls `SDL_AppQuit`. What
//! happens next depends on who called SDL:
//!
//! - **Desktop:** [`run`] gets control back and resumes the panic with
//!   [`std::panic::resume_unwind`], so it continues from `main` as usual.
//! - **iOS:** the process ends with `exit()` from inside UIKit's run loop, as SDL's own iOS
//!   entry ends it, and never returns to [`run`]. The quit callback aborts instead, so crash
//!   reporting sees a crash rather than a clean exit.
//! - **Android:** `SDL_main` is called from Java through C, so it can't unwind either;
//!   it aborts once SDL returns.

use std::any::Any;
use std::cell::{Cell, RefCell, UnsafeCell};
use std::collections::VecDeque;
use std::error::Error;
use std::ffi::{CString, c_char, c_int, c_void};
use std::fmt;
use std::panic::{self, AssertUnwindSafe};
use std::process::ExitCode;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use sdl3_sys::events::SDL_Event;
use sdl3_sys::init::{SDL_APP_CONTINUE, SDL_APP_FAILURE, SDL_APP_SUCCESS, SDL_AppResult};
use sdl3_sys::main::SDL_RunApp;
use sdl3_sys::timer::SDL_GetTicksNS;

use crate::event::TextEvent;
use crate::platform::{Platform, collect_args};
use crate::{Event, LOG_TARGET, sdl};

/// What the app wants after a callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AppControl {
    Continue,
    /// Shut down cleanly: SDL calls [`App::quit`] and the process exits successfully.
    Exit,
    /// Shut down because the app failed (for example, an unattended run that rendered
    /// nothing): SDL calls [`App::quit`] and the process exits with a failure status.
    Fail,
}

/// Identity of the app, set before SDL initializes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppMetadata {
    /// Human-readable name, for example `"Sapphire Sandbox"`. Also names the user
    /// directory.
    pub name: &'static str,
    /// Developer or publisher; the parent of the user directory on desktop.
    pub organization: &'static str,
    /// Reverse-DNS identifier, for example `"dev.sapphire.sandbox"`. Must match the
    /// iOS bundle identifier and Android application id.
    pub identifier: &'static str,
    pub version: &'static str,
}

/// A program driven by the platform's main loop.
///
/// Every method runs on the main thread. Calls never overlap: if the OS delivers a
/// lifecycle event while another callback is running (iOS can, for example, from
/// inside a modal system dialog), the platform queues it and delivers it as soon as
/// that callback returns.
///
/// A panic in any callback ends the app: SDL shuts down, dropping the app without
/// calling [`quit`](Self::quit). On desktop [`run`] then resumes the panic. On iOS and
/// Android, where SDL is driven from native code that can't unwind, the process
/// aborts so crash reporting records it.
pub trait App: Sized + 'static {
    /// Used to configure SDL and to locate [`Platform::user_dir`].
    const METADATA: AppMetadata;

    /// Called once, after SDL has initialized. Create the window here.
    fn init(platform: &mut Platform) -> Result<Self, AppInitError>;

    /// Called for each event, in order, before the next [`iterate`](Self::iterate).
    fn event(&mut self, platform: &mut Platform, event: &Event) -> AppControl;

    /// Called once per frame. On iOS and Android the engine starts each frame on a vsync at the
    /// rate set with [`Platform::set_frame_rate`] (on iOS from its display link, on Android from
    /// the choreographer's vsyncs), and not at all while an Android app is paused. On desktop
    /// SDL's loop calls it as fast as presentation allows (the renderer's present blocks for
    /// vsync).
    fn iterate(&mut self, platform: &mut Platform) -> AppControl;

    /// Called once before shutdown, unless a callback panicked. Not guaranteed on
    /// mobile, where the OS may kill a backgrounded app without notice; persist state
    /// on [`LifecycleEvent::WillEnterBackground`](crate::LifecycleEvent) instead.
    fn quit(&mut self, _platform: &mut Platform) {}
}

/// Why [`App::init`] failed. Converts from any error with `?`, or build one from a
/// message with [`AppInitError::msg`].
pub struct AppInitError(Box<dyn Error + Send + Sync + 'static>);

impl AppInitError {
    /// An error carrying only a message.
    pub fn msg(message: impl fmt::Display) -> Self {
        Self(message.to_string().into())
    }
}

impl<E: Error + Send + Sync + 'static> From<E> for AppInitError {
    fn from(error: E) -> Self {
        Self(Box::new(error))
    }
}

impl fmt::Display for AppInitError {
    /// The error followed by its chain of causes.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)?;
        let mut source = self.0.source();
        while let Some(cause) = source {
            write!(f, ": {cause}")?;
            source = cause.source();
        }
        Ok(())
    }
}

impl fmt::Debug for AppInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// How [`run_with`] starts the app.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunOptions {
    /// Run as a background process that never takes focus: on macOS the app gets no Dock
    /// icon, isn't brought to the foreground at launch, and windows it shows don't activate.
    /// For offscreen and automated runs (captures, tests) on a machine someone is using.
    /// Ignored on mobile.
    pub background: bool,
    /// Don't start SDL's video and gamepad subsystems: no windows and no display connection,
    /// which on macOS skips setting up AppKit (tens of milliseconds at launch and at exit).
    /// For offscreen tools that only render with `sapphire_rhi` and read content. Ignored on
    /// mobile, where the app always has a window.
    pub headless: bool,
}

/// Set by [`run_with`] before SDL starts; read when the hints are applied and SDL initializes.
static BACKGROUND: AtomicBool = AtomicBool::new(false);
static HEADLESS: AtomicBool = AtomicBool::new(false);

pub(crate) fn background_requested() -> bool {
    BACKGROUND.load(Ordering::Relaxed)
}

pub(crate) fn headless_requested() -> bool {
    HEADLESS.load(Ordering::Relaxed)
}

/// Counts the app runs that have ended. Each ends as its app quits, just before SDL shuts down
/// and destroys every window, view and display connection with it (review PA-005).
static ENDED_RUNS: AtomicU64 = AtomicU64::new(0);

/// The app run a window was made in. Windows and the surface handles taken from them remember
/// theirs and lend no native object once it has ended: SDL destroyed those objects as it shut
/// down, and a later run, which starts SDL again, doesn't bring them back.
///
/// Runs, windows and handles all stay on the main thread, so relaxed ordering is enough.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RunId(u64);

impl RunId {
    /// The run in progress.
    pub(crate) fn current() -> Self {
        Self(ENDED_RUNS.load(Ordering::Relaxed))
    }

    /// Whether this run is still in progress.
    pub(crate) fn is_current(self) -> bool {
        self == Self::current()
    }
}

/// Ends the run in progress, before SDL shuts down.
pub(crate) fn end_run() {
    ENDED_RUNS.fetch_add(1, Ordering::Relaxed);
}

/// Runs `A` on SDL's main callbacks and returns the process exit code.
///
/// Call it once, from `fn main` on the main thread (desktop and iOS). On iOS it hands
/// the thread to `UIApplicationMain` and never returns. Android doesn't call it: see
/// [`export_android_main!`](crate::export_android_main).
///
/// # Panics
///
/// Resumes any panic raised by an [`App`] callback, after SDL has shut down.
pub fn run<A: App>() -> ExitCode {
    run_with::<A>(RunOptions::default())
}

/// [`run`], with [`RunOptions`].
pub fn run_with<A: App>(options: RunOptions) -> ExitCode {
    BACKGROUND.store(options.background, Ordering::Relaxed);
    HEADLESS.store(options.headless, Ordering::Relaxed);
    // Pass the real arguments: on Unix, SDL would otherwise substitute a placeholder.
    let args: Vec<CString> =
        std::env::args_os().map(|arg| sdl::c_string_lossy(&arg.to_string_lossy())).collect();
    let mut argv: Vec<*mut c_char> =
        args.iter().map(|arg| arg.as_ptr().cast_mut()).chain([ptr::null_mut()]).collect();
    let argc = c_int::try_from(args.len()).unwrap_or(c_int::MAX);

    // SAFETY: `argv` is a null-terminated array of `argc` valid C strings that outlives
    // the call (on iOS the call never returns), and `sdl_main::<A>` has the signature
    // SDL expects of a main function.
    let status = unsafe { SDL_RunApp(argc, argv.as_mut_ptr(), Some(sdl_main::<A>), ptr::null_mut()) };

    if let Some(payload) = take_panic() {
        panic::resume_unwind(payload);
    }
    if status == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// Exports `SDL_main` for Android from the app's `cdylib`.
///
/// `init`, if given, names a `fn()` that runs first in `SDL_main`, before SDL starts:
/// the place for crash reporting (spec §5.2) and logging, which `fn main` sets up on
/// the other platforms. It must not panic (it runs inside an `extern "C"` function).
///
/// Expands to nothing on other targets, so the same `lib.rs` builds everywhere. See
/// the crate docs for the full Android packaging contract.
///
/// ```
/// use sapphire_platform::{App, AppControl, AppInitError, AppMetadata, Event, Platform};
///
/// struct Game;
///
/// impl App for Game {
///     const METADATA: AppMetadata = AppMetadata {
///         name: "Game",
///         organization: "Studio",
///         identifier: "com.studio.game",
///         version: "0.1.0",
///     };
///     fn init(_: &mut Platform) -> Result<Self, AppInitError> {
///         Ok(Game)
///     }
///     fn event(&mut self, _: &mut Platform, _: &Event) -> AppControl {
///         AppControl::Continue
///     }
///     fn iterate(&mut self, _: &mut Platform) -> AppControl {
///         AppControl::Continue
///     }
/// }
///
/// fn setup() {
///     // Install the crash handler and the logging subscriber.
/// }
///
/// // In the app crate's lib.rs (crate-type = ["cdylib", "lib"]):
/// sapphire_platform::export_android_main!(Game, init = setup);
/// # fn main() {}
/// ```
#[macro_export]
macro_rules! export_android_main {
    ($app:ty $(, init = $init:path)? $(,)?) => {
        #[cfg(target_os = "android")]
        const _: () = {
            // Export SDL's own C symbols, JNI_OnLoad above all, from this cdylib.
            // SDL is linked statically, and rustc otherwise hides every symbol a cdylib
            // doesn't define in Rust, so SDLActivity's native methods would never be
            // registered. `-bundle` links the archive sdl3-sys already built (it is on
            // the search path) instead of copying it into this crate.
            #[link(name = "SDL3", kind = "static", modifiers = "-bundle,+export-symbols")]
            unsafe extern "C" {}

            /// Entry point SDLActivity looks up with `dlsym` and calls on its SDL thread.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn SDL_main(
                argc: ::core::ffi::c_int,
                argv: *mut *mut ::core::ffi::c_char,
            ) -> ::core::ffi::c_int {
                $($init();)?
                // SAFETY: SDLActivity calls this once, on its SDL thread, with a valid
                // null-terminated `argv` of `argc` C strings.
                unsafe { $crate::__private::android_main::<$app>(argc, argv) }
            }
        };
    };
}

#[doc(hidden)]
pub mod __private {
    use super::*;

    /// The body of the `SDL_main` emitted by [`export_android_main!`].
    ///
    /// # Safety
    ///
    /// Must be called once, by SDL's Java glue (which has already run `SDL_RunApp`),
    /// with a valid `argv` of `argc` C strings.
    pub unsafe fn android_main<A: App>(argc: c_int, argv: *mut *mut c_char) -> c_int {
        // SAFETY: forwarded caller contract.
        let status = unsafe { sdl_main::<A>(argc, argv) };
        if take_panic().is_some() {
            abort_after_panic();
        }
        status
    }
}

/// The C main function SDL runs: hands control to SDL's callback loop.
unsafe extern "C" fn sdl_main<A: App>(argc: c_int, argv: *mut *mut c_char) -> c_int {
    // On iOS the engine's display link, not SDL's, drives the callbacks (ADR 0003), and on
    // Android the engine's vsync loop does, in place of SDL's back-to-back loop (ADR 0004).
    #[cfg(target_os = "android")]
    use crate::android_loop::enter_main_callbacks as enter;
    #[cfg(target_os = "ios")]
    use crate::ios_loop::enter_main_callbacks as enter;
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    use sdl3_sys::main::SDL_EnterAppMainCallbacks as enter;
    // SAFETY: `argc` / `argv` are forwarded unchanged from SDL_RunApp or SDLActivity,
    // and the four callbacks match SDL's signatures.
    unsafe {
        enter(
            argc,
            argv,
            Some(app_init::<A>),
            Some(app_iterate::<A>),
            Some(app_event::<A>),
            Some(app_quit::<A>),
        )
    }
}

unsafe extern "C" fn app_init<A: App>(
    appstate: *mut *mut c_void,
    argc: c_int,
    argv: *mut *mut c_char,
) -> SDL_AppResult {
    // SAFETY: SDL forwards the argv it was started with.
    let args = unsafe { collect_args(argc, argv) };
    let init = panic::catch_unwind(AssertUnwindSafe(|| -> Result<Box<Runtime<A>>, AppInitError> {
        let mut platform = Platform::new(&A::METADATA, args)?;
        let app = A::init(&mut platform)?;
        Ok(Box::new(Runtime::new(app, platform)))
    }));
    match init {
        Ok(Ok(runtime)) => {
            // SAFETY: SDL passes a valid out-pointer for the app state.
            unsafe { *appstate = Box::into_raw(runtime).cast::<c_void>() };
            SDL_APP_CONTINUE
        }
        Ok(Err(error)) => {
            tracing::error!(target: LOG_TARGET, "app initialization failed: {error}");
            SDL_APP_FAILURE
        }
        Err(payload) => {
            stash_panic(payload);
            SDL_APP_FAILURE
        }
    }
}

unsafe extern "C" fn app_iterate<A: App>(appstate: *mut c_void) -> SDL_AppResult {
    // SAFETY: SDL only iterates after `app_init` succeeded, passing back the
    // `Runtime<A>` it installed, and never after `app_quit` freed it.
    let runtime = unsafe { &*appstate.cast::<Runtime<A>>() };
    runtime.iterate()
}

unsafe extern "C" fn app_event<A: App>(appstate: *mut c_void, event: *mut SDL_Event) -> SDL_AppResult {
    // SAFETY: as in `app_iterate`; `event` is valid for the duration of the call.
    let (runtime, event) = unsafe { (&*appstate.cast::<Runtime<A>>(), &*event) };
    runtime.event(event)
}

unsafe extern "C" fn app_quit<A: App>(appstate: *mut c_void, _result: SDL_AppResult) {
    // Null if `app_init` failed.
    if !appstate.is_null() {
        // SAFETY: `appstate` is the `Runtime<A>` installed by `app_init`, still alive.
        if unsafe { &*appstate.cast::<Runtime<A>>() }.busy.get() {
            // SDL is quitting from inside one of our callbacks: on iOS a nested run loop
            // (a system alert, say) can deliver TERMINATING while `iterate` runs, and SDL
            // then quits and calls exit() without returning to it. That callback still
            // holds `&mut State`, so dropping the state here would free it underneath a
            // live borrow. Leak it instead; the process is about to end.
            tracing::warn!(target: LOG_TARGET, "quit requested inside a callback; app state not dropped");
        } else {
            // SAFETY: `appstate` is the box leaked by `app_init`; no callback is running,
            // SDL calls quit exactly once and never uses the pointer afterwards.
            let runtime = unsafe { Box::from_raw(appstate.cast::<Runtime<A>>()) };
            shut_down(runtime.state.into_inner());
        }
    }
    // SDL shuts down right after this callback, so whatever the app kept past its run (in a
    // thread-local, say) must stop lending native handles now.
    end_run();
    if cfg!(target_os = "ios") && has_panicked() {
        // The process exits right after this; see the module docs.
        abort_after_panic();
    }
}

/// Runs [`App::quit`] (unless a callback panicked) and drops the app, then the platform.
fn shut_down<A: App>(mut state: State<A>) {
    if !has_panicked() {
        let quit = panic::catch_unwind(AssertUnwindSafe(|| state.app.quit(&mut state.platform)));
        if let Err(payload) = quit {
            stash_panic(payload);
        }
    }
    // Dropping the app destroys its windows, then the platform closes gamepads, all
    // before SDL_Quit runs.
    if let Err(payload) = panic::catch_unwind(AssertUnwindSafe(|| drop(state))) {
        stash_panic(payload);
    }
}

struct State<A> {
    // Declared first so it drops first: the app's windows go before the platform.
    app: A,
    platform: Platform,
}

/// The app state SDL carries between callbacks, plus the guard that keeps callback
/// borrows exclusive.
struct Runtime<A> {
    state: UnsafeCell<State<A>>,
    /// Set while a callback is running.
    busy: Cell<bool>,
    /// Events that arrived while `busy` (SDL dispatches lifecycle events synchronously,
    /// from wherever the OS raised them).
    deferred: RefCell<VecDeque<SDL_Event>>,
}

impl<A: App> Runtime<A> {
    fn new(app: A, platform: Platform) -> Self {
        Self {
            state: UnsafeCell::new(State { app, platform }),
            busy: Cell::new(false),
            deferred: RefCell::new(VecDeque::new()),
        }
    }

    fn iterate(&self) -> SDL_AppResult {
        if self.busy.get() {
            // A frame requested from inside another callback is skipped, not nested.
            return SDL_APP_CONTINUE;
        }
        self.exclusive(|state| {
            // The frame clock turns down a display tick that came sooner than the target interval.
            if !state.platform.begin_frame() {
                return AppControl::Continue;
            }
            state.app.iterate(&mut state.platform)
        })
    }

    fn event(&self, raw: &SDL_Event) -> SDL_AppResult {
        let mut raw = *raw;
        // SAFETY: every event variant starts with the common header, so `timestamp` is
        // initialized whatever the variant.
        if unsafe { raw.common.timestamp } == 0 {
            // Lifecycle events bypass SDL's queue, which is what stamps events, and reach
            // us synchronously as they happen. Stamp them on arrival, before any deferral.
            // SAFETY: SDL_GetTicksNS has no preconditions.
            raw.common.timestamp = unsafe { SDL_GetTicksNS() };
        }
        if self.busy.get() {
            self.deferred.borrow_mut().push_back(raw);
            return SDL_APP_CONTINUE;
        }
        self.exclusive(|state| state.dispatch(&raw))
    }

    /// Runs `f` with the only reference to the state, then drains events deferred
    /// meanwhile.
    fn exclusive(&self, f: impl FnOnce(&mut State<A>) -> AppControl) -> SDL_AppResult {
        self.busy.set(true);
        // SAFETY: all callbacks run on SDL's main thread, and every path that reaches
        // the state checks `busy` first, so this is the only live reference until
        // `busy` is cleared below.
        let state = unsafe { &mut *self.state.get() };
        let mut result = guarded(|| f(state));
        while result == SDL_APP_CONTINUE {
            let Some(raw) = self.deferred.borrow_mut().pop_front() else { break };
            result = guarded(|| state.dispatch(&raw));
        }
        self.busy.set(false);
        result
    }
}

impl<A: App> State<A> {
    fn dispatch(&mut self, raw: &SDL_Event) -> AppControl {
        if let Some((timestamp, window, text)) = crate::event::text_input(raw) {
            for chunk in TextEvent::split(timestamp, window, &text) {
                let control = self.app.event(&mut self.platform, &Event::Text(chunk));
                if control != AppControl::Continue {
                    return control;
                }
            }
            return AppControl::Continue;
        }
        match self.platform.translate_event(raw) {
            Some(event) => self.app.event(&mut self.platform, &event),
            None => AppControl::Continue,
        }
    }
}

/// Runs an app callback, converting its result to SDL's and catching panics.
fn guarded(f: impl FnOnce() -> AppControl) -> SDL_AppResult {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(AppControl::Continue) => SDL_APP_CONTINUE,
        Ok(AppControl::Exit) => SDL_APP_SUCCESS,
        Ok(AppControl::Fail) => SDL_APP_FAILURE,
        Err(payload) => {
            stash_panic(payload);
            SDL_APP_FAILURE
        }
    }
}

thread_local! {
    /// The first panic raised by an app callback. Callbacks all run on one thread.
    static PANIC: RefCell<Option<Box<dyn Any + Send>>> = const { RefCell::new(None) };
}

fn stash_panic(payload: Box<dyn Any + Send>) {
    // Later panics (say, in `drop` after a failed frame) are usually consequences of
    // the first; the panic hook has already reported them.
    PANIC.with_borrow_mut(|slot| {
        slot.get_or_insert(payload);
    });
}

fn has_panicked() -> bool {
    PANIC.with_borrow(Option::is_some)
}

fn take_panic() -> Option<Box<dyn Any + Send>> {
    PANIC.with_borrow_mut(Option::take)
}

fn abort_after_panic() -> ! {
    tracing::error!(target: LOG_TARGET, "an app callback panicked (reported above); aborting");
    std::process::abort()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("outer")]
    struct Outer(#[source] std::io::Error);

    #[test]
    fn init_error_shows_the_cause_chain() {
        let error = AppInitError::from(Outer(std::io::Error::other("disk on fire")));
        assert_eq!(error.to_string(), "outer: disk on fire");
        assert_eq!(AppInitError::msg("no GPU").to_string(), "no GPU");
    }

    #[test]
    fn init_error_converts_with_question_mark() {
        fn init() -> Result<(), AppInitError> {
            Err(std::io::Error::other("missing content"))?;
            Ok(())
        }
        assert_eq!(init().unwrap_err().to_string(), "missing content");
    }

    #[test]
    fn only_the_first_panic_is_kept() {
        assert!(!has_panicked());
        assert_eq!(guarded(|| panic!("first")).0, SDL_APP_FAILURE.0);
        assert_eq!(guarded(|| panic!("second")).0, SDL_APP_FAILURE.0);
        let payload = take_panic().expect("a panic was stashed");
        assert_eq!(payload.downcast_ref::<&str>(), Some(&"first"));
        assert!(!has_panicked());
    }

    #[test]
    fn app_control_maps_to_sdl_results() {
        assert_eq!(guarded(|| AppControl::Continue).0, SDL_APP_CONTINUE.0);
        assert_eq!(guarded(|| AppControl::Exit).0, SDL_APP_SUCCESS.0);
        assert_eq!(guarded(|| AppControl::Fail).0, SDL_APP_FAILURE.0);
    }
}
