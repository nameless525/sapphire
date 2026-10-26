//! The [`Platform`] handle passed to every [`App`](crate::App) callback.

use std::ffi::c_char;
use std::marker::PhantomData;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::time::Duration;

use sdl3_sys::events::SDL_Event;
use sdl3_sys::filesystem::SDL_GetBasePath;
use sdl3_sys::gamepad::{SDL_CloseGamepad, SDL_Gamepad, SDL_GetGamepadType, SDL_OpenGamepad};
use sdl3_sys::hints::{
    SDL_HINT_IOS_HIDE_HOME_INDICATOR, SDL_HINT_MAC_BACKGROUND_APP, SDL_HINT_MOUSE_TOUCH_EVENTS,
    SDL_HINT_ORIENTATIONS, SDL_HINT_PEN_MOUSE_EVENTS, SDL_HINT_PEN_TOUCH_EVENTS, SDL_HINT_TOUCH_MOUSE_EVENTS,
    SDL_HINT_WINDOW_ACTIVATE_WHEN_SHOWN,
};
use sdl3_sys::init::{SDL_INIT_EVENTS, SDL_INIT_GAMEPAD, SDL_INIT_VIDEO, SDL_Init, SDL_SetAppMetadata};
use sdl3_sys::joystick::SDL_JoystickID;
use sdl3_sys::keyboard::SDL_GetKeyName;
use sdl3_sys::keycode::SDL_Keycode;
use sdl3_sys::timer::SDL_GetTicksNS;
use sdl3_sys::video::SDL_GetCurrentVideoDriver;

use crate::content::{self, ContentFs};
use crate::dirs;
use crate::event::{self, Event, GamepadEvent, GamepadId, GamepadType, Keycode, LifecycleEvent, Timestamp};
use crate::frame_clock::{FrameClock, FramePacing};
use crate::system::SystemInfo;
use crate::window::{Window, WindowDesc};
use crate::{AppMetadata, LOG_TARGET, sdl};

/// Errors from platform calls.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PlatformError {
    /// An SDL call failed; `message` is SDL's error string.
    #[error("{call} failed: {message}")]
    Sdl { call: &'static str, message: String },
    /// A platform directory couldn't be created.
    #[error("cannot create {}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Access to the OS, handed to every [`App`](crate::App) callback.
///
/// SDL's window and event APIs must be called from the main thread, so `Platform` (like
/// [`Window`]) is neither `Send` nor `Sync`. Things that are safe off the main thread,
/// such as [`ContentFs`] and [`sdl_log`](crate::sdl_log), are separate types.
#[derive(Debug)]
pub struct Platform {
    args: Vec<String>,
    user_dir: PathBuf,
    cache_dir: PathBuf,
    content: ContentFs,
    system_info: SystemInfo,
    /// Gamepads opened on `GAMEPAD_ADDED`; SDL only reports input for open gamepads.
    gamepads: Vec<(GamepadId, NonNull<SDL_Gamepad>)>,
    last_background_transition: Option<LifecycleEvent>,
    frame_clock: FrameClock,
    _main_thread_only: PhantomData<*mut ()>,
}

impl Platform {
    /// Configures and initializes SDL. Runs inside `SDL_AppInit` on the main thread.
    pub(crate) fn new(metadata: &AppMetadata, args: Vec<String>) -> Result<Self, PlatformError> {
        apply_hints();

        let name = sdl::c_string_lossy(metadata.name);
        let version = sdl::c_string_lossy(metadata.version);
        let identifier = sdl::c_string_lossy(metadata.identifier);
        // SAFETY: all three are valid C strings, which SDL copies.
        if !unsafe { SDL_SetAppMetadata(name.as_ptr(), version.as_ptr(), identifier.as_ptr()) } {
            tracing::warn!(target: LOG_TARGET, error = %sdl::last_error(), "SDL_SetAppMetadata failed");
        }

        // Video brings in events, keyboard, mouse and touch; gamepad brings in joystick.
        // Haptic and sensor are compiled in but not initialized until their APIs exist.
        // Headless runs (`RunOptions::headless`) need only the event loop.
        let headless =
            crate::app::headless_requested() && !cfg!(any(target_os = "ios", target_os = "android"));
        let subsystems = if headless { SDL_INIT_EVENTS } else { SDL_INIT_VIDEO | SDL_INIT_GAMEPAD };
        // SAFETY: called once, on the main thread, from SDL_AppInit.
        if !unsafe { SDL_Init(subsystems) } {
            return Err(sdl::error("SDL_Init"));
        }

        let user_dir = dirs::user_dir(metadata)?;
        let cache_dir = dirs::cache_dir(metadata)?;
        // SAFETY: SDL_GetBasePath returns null or a string SDL caches for the process.
        let base_path = unsafe { sdl::borrowed_string(SDL_GetBasePath()) };
        let content = ContentFs::new(content::default_root(base_path));
        content::warm_up();
        let system_info = SystemInfo::query();

        // SAFETY: returns null or a static driver name.
        let video_driver = unsafe { sdl::borrowed_string(SDL_GetCurrentVideoDriver()) };
        tracing::info!(
            target: LOG_TARGET,
            video_driver = video_driver.as_deref().unwrap_or("none"),
            content_root = content.root(),
            user_dir = %user_dir.display(),
            cache_dir = %cache_dir.display(),
            ?system_info,
            "platform initialized",
        );

        Ok(Self {
            args,
            user_dir,
            cache_dir,
            content,
            system_info,
            gamepads: Vec::new(),
            last_background_transition: None,
            frame_clock: FrameClock::new(),
            _main_thread_only: PhantomData,
        })
    }

    /// Command-line arguments, including the program name. On Android these come from
    /// `SDLActivity.getArguments()`, since the process has no command line of its own.
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// Creates an OS window. On mobile it is always fullscreen.
    pub fn create_window(&self, desc: &WindowDesc) -> Result<Window, PlatformError> {
        Window::create(desc)
    }

    /// The writable directory for settings and saves; the same path as
    /// [`crate::user_dir`], which documents where it lives.
    pub fn user_dir(&self) -> &Path {
        &self.user_dir
    }

    /// The writable, never-backed-up directory for pipeline caches, logs and crash
    /// dumps; the same path as [`crate::cache_dir`], which documents where it lives.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// The read-only packaged content.
    pub fn content(&self) -> &ContentFs {
        &self.content
    }

    pub fn system_info(&self) -> &SystemInfo {
        &self.system_info
    }

    /// The current time on the clock event timestamps use.
    pub fn now(&self) -> Timestamp {
        // SAFETY: SDL_GetTicksNS has no preconditions.
        Timestamp::from_nanos(unsafe { SDL_GetTicksNS() })
    }

    /// Sets the frame rate the app is built for (spec §6.2), normally its quality tier's; `None`
    /// for the display's own rate. Takes effect from the next frame.
    ///
    /// On iOS the engine's display link asks the display for exactly this rate, so a ProMotion
    /// panel can run at it rather than 120 Hz, and starts each frame on a vsync, holding back any
    /// vsync that comes before the frame is due (ADR 0003). On desktop and Android,
    /// where presentation paces frames, it is what [`Self::frame_pacing`] measures against.
    pub fn set_frame_rate(&mut self, rate: Option<NonZeroU32>) {
        self.frame_clock.set_frame_rate(rate);
    }

    /// How frames are paced and how evenly they have come (rule R9).
    pub fn frame_pacing(&self) -> FramePacing {
        self.frame_clock.pacing()
    }

    /// The current frame's interval since the frame before where a display link paces frames
    /// (iOS): the time between the vsyncs the two are shown at, so steady frames advance the
    /// simulation and animation by exactly the target interval. In the background it is at most
    /// one target interval. `None` where presentation paces frames; measure with
    /// `sapphire_time`'s `RealClock` there.
    pub fn frame_delta(&self) -> Option<Duration> {
        self.frame_clock.display_interval()
    }

    /// Runs before each [`App::iterate`](crate::App::iterate): whether this tick becomes a frame.
    pub(crate) fn begin_frame(&mut self) -> bool {
        self.frame_clock.begin_frame()
    }

    /// A human-readable name for a key under the current layout (for rebinding UI and
    /// prompts), or an empty string if it has none.
    pub fn key_name(&self, key: Keycode) -> String {
        // SAFETY: SDL_GetKeyName reads static tables and SDL's global keyboard state, which
        // falls back to the default keymap without video (headless runs); the returned string
        // is copied before any other SDL call.
        unsafe { sdl::borrowed_string(SDL_GetKeyName(SDL_Keycode(key.0))) }.unwrap_or_default()
    }

    /// Translates an SDL event and keeps platform-owned state in step with it.
    pub(crate) fn translate_event(&mut self, raw: &SDL_Event) -> Option<Event> {
        self.frame_clock.observe(raw);
        let event = event::translate(raw)?;
        match event {
            // A gamepad that fails to open sends no input, so it isn't announced.
            Event::Gamepad { timestamp, gamepad, event: GamepadEvent::Added { .. } } => {
                let kind = self.open_gamepad(gamepad)?;
                return Some(Event::Gamepad { timestamp, gamepad, event: GamepadEvent::Added { kind } });
            }
            Event::Gamepad { gamepad, event: GamepadEvent::Removed, .. } => self.close_gamepad(gamepad),
            Event::Lifecycle { event: transition, .. } if self.repeats_last_transition(transition) => {
                return None;
            }
            // Frames between leaving the foreground and coming back may stall or sleep, so their
            // intervals aren't measured.
            Event::Lifecycle { event: LifecycleEvent::WillEnterBackground, .. } => self.frame_clock.pause(),
            Event::Lifecycle { event: LifecycleEvent::DidEnterForeground, .. } => self.frame_clock.resume(),
            _ => {}
        }
        Some(event)
    }

    /// Records a background / foreground transition and reports whether it repeats the
    /// previous one. SDL's UIKit backend sends some transitions twice (from both the app
    /// delegate and its lifecycle observer); apps see each change once.
    fn repeats_last_transition(&mut self, transition: LifecycleEvent) -> bool {
        transition.is_background_transition()
            && self.last_background_transition.replace(transition) == Some(transition)
    }

    /// Opens the gamepad, once, and returns its type; `None` if it can't be opened.
    fn open_gamepad(&mut self, id: GamepadId) -> Option<GamepadType> {
        let gamepad = match self.gamepads.iter().find(|&&(open, _)| open == id) {
            Some(&(_, gamepad)) => gamepad,
            // SAFETY: the gamepad subsystem is initialized and `id` came from SDL.
            None => match NonNull::new(unsafe { SDL_OpenGamepad(SDL_JoystickID(id.0)) }) {
                Some(gamepad) => {
                    self.gamepads.push((id, gamepad));
                    gamepad
                }
                None => {
                    tracing::warn!(target: LOG_TARGET, ?id, error = %sdl::last_error(), "failed to open gamepad");
                    return None;
                }
            },
        };
        // SAFETY: every gamepad in the list is open until its removal closes it.
        let kind = GamepadType::from_sdl(unsafe { SDL_GetGamepadType(gamepad.as_ptr()) });
        tracing::info!(target: LOG_TARGET, ?id, ?kind, "gamepad connected");
        Some(kind)
    }

    fn close_gamepad(&mut self, id: GamepadId) {
        if let Some(index) = self.gamepads.iter().position(|&(open, _)| open == id) {
            let (_, gamepad) = self.gamepads.swap_remove(index);
            // SAFETY: opened by `open_gamepad` and removed from the list, so closed once.
            unsafe { SDL_CloseGamepad(gamepad.as_ptr()) };
        }
    }
}

impl Drop for Platform {
    fn drop(&mut self) {
        // Runs in SDL_AppQuit, before SDL_Quit.
        for (_, gamepad) in self.gamepads.drain(..) {
            // SAFETY: each gamepad in the list is open and is closed exactly once here.
            unsafe { SDL_CloseGamepad(gamepad.as_ptr()) };
        }
    }
}

/// Hints that must be set before SDL initializes.
fn apply_hints() {
    // Each input device is reported once, through one event family (see `TouchEvent`):
    // touches don't fake mouse clicks, the mouse doesn't fake touches, and the pen is a
    // finger on its own touch device rather than a mouse.
    sdl::set_hint(SDL_HINT_TOUCH_MOUSE_EVENTS, c"0");
    sdl::set_hint(SDL_HINT_MOUSE_TOUCH_EVENTS, c"0");
    sdl::set_hint(SDL_HINT_PEN_MOUSE_EVENTS, c"0");
    sdl::set_hint(SDL_HINT_PEN_TOUCH_EVENTS, c"1");

    if crate::app::background_requested() {
        // Never take focus: no foreground launch on macOS, and windows don't activate when
        // shown (see `RunOptions::background`).
        sdl::set_hint(SDL_HINT_MAC_BACKGROUND_APP, c"1");
        sdl::set_hint(SDL_HINT_WINDOW_ACTIVATE_WHEN_SHOWN, c"0");
    }
    if cfg!(any(target_os = "ios", target_os = "android")) {
        // Landscape only (spec §5.1). The Info.plist / AndroidManifest must allow these
        // orientations too, or the OS overrides the hint.
        sdl::set_hint(SDL_HINT_ORIENTATIONS, c"LandscapeLeft LandscapeRight");
    }
    if cfg!(target_os = "ios") {
        // "2": the home indicator is dimmed and the first swipe only reveals it, so
        // edge swipes during combat don't leave the game.
        sdl::set_hint(SDL_HINT_IOS_HIDE_HOME_INDICATOR, c"2");
    }
}

/// Copies `argc` / `argv` as SDL hands them to `SDL_AppInit`.
///
/// # Safety
///
/// `argv` must be null or point to at least `argc` pointers, each null or a valid
/// NUL-terminated string.
pub(crate) unsafe fn collect_args(argc: i32, argv: *mut *mut c_char) -> Vec<String> {
    if argv.is_null() {
        return Vec::new();
    }
    (0..usize::try_from(argc).unwrap_or(0))
        .filter_map(|i| {
            // SAFETY: the caller guarantees `argv` has at least `argc` entries, each a
            // valid C string or null.
            unsafe { sdl::borrowed_string(*argv.add(i)) }
        })
        .collect()
}
