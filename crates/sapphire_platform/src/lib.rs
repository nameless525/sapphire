//! SDL3-based platform layer: app lifecycle, windows, surfaces, input events, content
//! and user file systems (spec §5.1, layer L1).
//!
//! # Owns
//!
//! - **Entry and lifecycle** through SDL's main callbacks on every platform: [`run`]
//!   from `fn main` on desktop and iOS, [`export_android_main!`] on Android. The app
//!   implements [`App`]. On iOS the engine's display link drives them, and on Android the
//!   engine's vsync loop (see "Frame pacing").
//! - **The OS window** ([`Window`]) and the [`SurfaceHandle`] the renderer builds its
//!   surface from: a `CAMetalLayer` on Apple platforms, `raw-window-handle` 0.6 handles
//!   elsewhere, kept alive for as long as the surface needs them.
//! - **Raw input and OS events** as engine-owned [`Event`]s, each with SDL's
//!   nanosecond [`Timestamp`]. Mapping them to actions is `sapphire_input`'s job.
//! - **Text input** (M1 F5): [`Window::start_text_input`] turns on [`Event::Text`] (typed and
//!   input-method text) and the on-screen keyboard on mobile, for a focused text field;
//!   [`TextInputKind`] says whether the keyboard may capitalize and autocorrect.
//! - **Content and user storage:** [`ContentFs`] for packaged read-only content
//!   (including APK assets), [`user_dir`] for settings and saves, and [`cache_dir`] for
//!   rebuildable data such as pipeline caches, logs and crash dumps. Both directory
//!   functions work before SDL starts. [`durable_replace`] writes a file so that a crash leaves
//!   the old one or the new one whole (M3 V1).
//! - **Patches' side of the disk** (M4 P4): [`ContentFs::with_files_over`] reads a patch's packs and
//!   files in place of what the app shipped, [`free_space`] says how much room a disk has left, and
//!   [`exclude_from_backup`] keeps a directory the game can fetch again out of iCloud and the
//!   computer's backups on iOS.
//! - **The IO thread** (M1 A4, [`IoThread`]): positioned reads of content files by priority,
//!   with cancellation and completion callbacks (spec §4.3). The asset server reads packs
//!   through it; decoding happens on the job system's workers, not here.
//! - **The OS's side of sound** (M2 E1): iOS's audio session and interruptions and Android's context
//!   under SDL's entry ([`Platform::prepare_audio`], [`Platform::audio_interrupted`]).
//! - **A gamepad's rumble** (M2 E3, [`Platform::rumble_gamepad`]), since it holds the open pads.
//! - **Device facts** ([`SystemInfo`]) and a native log sink ([`sdl_log`]) for mobile.
//! - **Frame pacing** (M1 F7, F8): when [`App::iterate`] runs, at the rate the app sets with
//!   [`Platform::set_frame_rate`], and how evenly frames come ([`FramePacing`]). See below. On
//!   Android it also reports each frame's work to an ADPF hint session for the frame thread (M4 T1),
//!   less its swapchain waits (M4 T7, [`FrameWork`]).
//! - **Heat and power** (M4 T1): [`Platform::thermal`]'s [`ThermalReading`] a frame, from each
//!   source the OS has (Android's thermal status, headroom and hint session, iOS's thermal state
//!   and Low Power Mode), and the [`CVARS`] that force one.
//!
//! # Does not own
//!
//! GPU devices and swapchains (`sapphire_rhi`), mixing and streams (`sapphire_audio`), logging setup
//! (the app wires `tracing` to [`sdl_log`]), packs and what is in them (`sapphire_assets`), and,
//! until their milestones, IME composition text (shown while composing; committed text already
//! arrives), a phone's haptics and clipboard. Answering heat is the quality ladder's
//! (`sapphire_render::quality`): this crate only reads it.
//!
//! # Later milestones
//!
//! - Memory-mapping pack files where the OS allows (desktop, iOS, and packs downloaded into
//!   Android's internal storage, spec §6.4), so raw blocks are read in place; and the IO
//!   thread's priority above normal, with the job system's thread priorities (spec §5.5).
//! - Desktop's player frame cap (spec §6.2) on the same pacer. Android's loop has the shape it
//!   needs (the engine waits before each iteration and the pacer gates it), but a desktop has no
//!   vsync to wait for through SDL: the cap would sleep to the time the pacer says the next frame
//!   is due, on the clock, and let FIFO's present keep frames on vsyncs.
//! - Timed presentation once wgpu can present at a chosen time (Metal's present time on iOS,
//!   `VK_GOOGLE_display_timing` on Android), so frames that finish early on a panel faster than
//!   the target are still shown at even intervals.
//!
//! # Threading
//!
//! SDL runs every callback on the main thread (on Android, SDLActivity's SDL thread),
//! so [`Platform`], [`Window`] and [`SurfaceHandle`] are `!Send`. [`ContentFs`],
//! [`ContentFile`], [`IoThread`] and [`sdl_log`] may be used from any thread. This crate never exposes
//! SDL types or raw SDL pointers; the only pointers it hands out are the native surface
//! handles.
//!
//! # Frame pacing
//!
//! The app sets the frame rate it is built for (the quality tier's) with
//! [`Platform::set_frame_rate`]; what that does depends on what starts frames (spec §6.2,
//! ADR 0003 for iOS, ADR 0004 for Android):
//!
//! - **iOS:** the engine's `CADisplayLink` drives SDL's main callbacks in place of SDL's own link,
//!   which ran at the display's full rate (up to 120 Hz on ProMotion). The link asks the display
//!   for exactly the target rate, so a ProMotion panel can drop to it, and each frame starts on a
//!   vsync; a tick that comes before a frame is due is held back. SDL's link for each window,
//!   which asked for 80–120 Hz, is stopped. Event dispatch, lifecycle events and quitting stay
//!   SDL's, through SDL's own main-callback functions. [`Platform::frame_delta`] is the time
//!   between the vsyncs frames are shown at, for the simulation and animation.
//! - **Android:** the engine's loop runs SDL's main callbacks in place of SDL's back-to-back loop,
//!   waiting before each iteration for a vsync from the NDK's `AChoreographer`, on the SDL thread's
//!   own looper (the thread sleeps between vsyncs). The pacer starts a frame on the vsyncs the
//!   target rate is due on and holds back the rest, so a 30 fps tier draws 30 frames a second
//!   whatever the panel's rate. The windows the app renders to ask the display for the target rate
//!   (`ANativeWindow_setFrameRate`, Android 11 and later), and the choreographer reports the
//!   refresh as it changes. Event dispatch, lifecycle events and quitting stay SDL's.
//!   [`Platform::frame_delta`] is the time between the vsyncs frames start on.
//! - **Desktop:** SDL's loop runs frames back to back and presentation (FIFO) waits for vsync, as
//!   before; the target is what the stats measure against.
//!
//! [`Platform::frame_pacing`] reports the source, the target, the display's refresh and the
//! counters (on time, missed, doubled, held back) for stats panels.
//!
//! The decisions are a pure function of the ticks' vsync times (`pacing.rs`), unit-tested with
//! injected timestamps. They count whole refreshes, so the target rate holds, never exceeded, on
//! panels it doesn't divide (60 fps at 90 or 144 Hz). Frames start on time; when each is shown is
//! presentation's (FIFO's) call. Presenting at a chosen time (Metal's minimum present duration,
//! `CAMetalDisplayLink`, Vulkan's present timing) would hold intervals even for a frame that
//! finishes early on a panel faster than the target, and waits for wgpu to expose it.
//!
//! # Android packaging
//!
//! SDL's Java glue drives the app, so an Android build needs:
//!
//! 1. The app crate builds as a `cdylib`, say `libsandbox.so`, whose `lib.rs` invokes
//!    `sapphire_platform::export_android_main!(SandboxApp, init = setup);`, adding
//!    `offscreen = OffscreenApp, when = asks_offscreen` for an app that runs offscreen
//!    when the intent's arguments ask, as `fn main` picks one elsewhere. That
//!    exports `SDL_main(argc, argv)`, which SDLActivity looks up with `dlsym` and calls
//!    on its SDL thread once the surface is ready, plus all of SDL's C symbols,
//!    including `JNI_OnLoad`, which registers SDL's Java native methods. SDL and the
//!    NDK's static libc++ are linked into that one library.
//! 2. Building SDL for Android needs the NDK's CMake toolchain file, for example
//!    `CMAKE_TOOLCHAIN_FILE_aarch64_linux_android=$NDK/build/cmake/android.toolchain.cmake`,
//!    alongside the usual `CC_*`, `CXX_*`, `AR_*` and linker variables for the target.
//! 3. The Gradle project compiles SDL's Java sources, `org.libsdl.app.*`, from the
//!    `sdl3-src` crate (`SDL/android-project/app/src/main/java/org/libsdl/app/` in the
//!    cargo registry, matching the `sdl3-sys` version in `Cargo.lock`).
//! 4. The activity extends `SDLActivity` and overrides `getLibraries()` to return
//!    exactly `{ "sandbox" }`: there is no separate `libSDL3.so`. `getMainFunction()`
//!    keeps its default, `"SDL_main"`.
//! 5. The manifest locks the activity to landscape, matching the orientation hint set
//!    here.
//! 6. Packaged content goes in the APK's `assets/content/` directory (see
//!    [`ContentFs`]).

// SDL's Android HIDAPI backend is C++, so the C++ runtime must be linked. The NDK's
// static libc++ keeps the app a single self-contained `.so`. The NDK allows only one
// C++ runtime per process: if a prebuilt C++ library that needs `libc++_shared.so`
// joins the APK, switch this to `c++_shared` and package that `.so`.
#[cfg(target_os = "android")]
#[link(name = "c++_static", kind = "static", modifiers = "-bundle")]
unsafe extern "C" {}
#[cfg(target_os = "android")]
#[link(name = "c++abi", kind = "static", modifiers = "-bundle")]
unsafe extern "C" {}

/// `tracing` target for everything this crate logs.
const LOG_TARGET: &str = "sapphire::platform";

#[cfg(target_os = "android")]
mod android_loop;
mod app;
mod audio_session;
mod content;
mod dirs;
mod disk;
mod durable;
mod event;
mod frame_clock;
mod hint;
mod host;
mod io;
#[cfg(target_os = "ios")]
mod ios_loop;
mod log;
mod memory;
mod pacing;
mod platform;
mod sdl;
mod system;
mod thermal;
mod window;

#[doc(hidden)]
pub use app::__private;
pub use app::{App, AppControl, AppInitError, AppMetadata, RunOptions, run, run_with};
pub use content::{CONTENT_DIR_ENV, ContentError, ContentFile, ContentFs};
pub use dirs::{cache_dir, user_dir};
pub use disk::{exclude_from_backup, free_space};
pub use durable::{OsSteps, ReplaceSteps, durable_replace, durable_replace_with};
pub use event::{
    Event, FingerId, GamepadAxis, GamepadButton, GamepadEvent, GamepadId, GamepadType, KeyEvent, Keycode,
    LifecycleEvent, Modifiers, MouseButton, MouseButtonEvent, MouseMotionEvent, MouseWheelEvent, Scancode,
    TextEvent, Timestamp, TouchDeviceId, TouchEvent, TouchPhase, WindowEvent,
};
pub use frame_clock::{FramePacing, FrameWork};
pub use host::Host;
pub use io::{ContentReader, IoFile, IoPriority, IoThread, IoTicket, ReadCallback, ReadError};
pub use log::{PlatformLogSink, sdl_log};
pub use memory::{ProcessMemory, process_memory};
pub use pacing::{PacingSource, PacingStats};
pub use platform::{Platform, PlatformError};
pub use raw_window_handle;
pub use system::{DeviceClass, Os, SystemInfo};
pub use thermal::{
    CVARS, FORCE_HEADROOM, FORCE_LOW_POWER, FORCE_STATE, ForcedState, ForcedSwitch, ThermalReading,
    ThermalState,
};
pub use window::{
    Rect, Size, SurfaceHandle, TextInputKind, Window, WindowDesc, WindowId, points_per_coordinate,
};
