//! Engine-owned events translated from SDL.
//!
//! Nothing here exposes an SDL type. Every event carries the [`Timestamp`] at which SDL
//! observed it, on the same clock as [`Platform::now`](crate::Platform::now), so input
//! buffering can measure the age of an input precisely rather than rounding it to the
//! frame it was pumped in.

use std::ffi::CStr;
use std::fmt;
use std::time::Duration;

use sdl3_sys::events::{SDL_Event, SDL_EventType};
use sdl3_sys::gamepad::{SDL_GamepadAxis, SDL_GamepadButton, SDL_GamepadType};
use sdl3_sys::keycode::SDL_Keymod;
use sdl3_sys::mouse::SDL_MOUSEWHEEL_FLIPPED;
use sdl3_sys::pen::SDL_PEN_TOUCHID;

use crate::window::WindowId;

/// A point on SDL's monotonic clock, in nanoseconds since SDL initialized.
///
/// Event timestamps and [`Platform::now`](crate::Platform::now) share this clock, so
/// they can be compared directly. It is unrelated to wall-clock time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(u64);

impl Timestamp {
    /// A timestamp `nanos` nanoseconds after SDL initialized.
    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    /// Nanoseconds since SDL initialized.
    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    /// Time elapsed from `earlier` to `self`, or zero if `earlier` is later.
    pub const fn saturating_duration_since(self, earlier: Timestamp) -> Duration {
        Duration::from_nanos(self.0.saturating_sub(earlier.0))
    }
}

/// An event delivered to [`App::event`](crate::App::event).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum Event {
    /// The user or OS asked the app to quit (on desktop, also sent after the last
    /// window's close request).
    Quit { timestamp: Timestamp },
    /// An OS lifecycle transition. See [`LifecycleEvent`] for what each one obliges
    /// the engine to do.
    Lifecycle { timestamp: Timestamp, event: LifecycleEvent },
    /// Something happened to an OS window.
    Window { timestamp: Timestamp, window: WindowId, event: WindowEvent },
    /// A key was pressed, auto-repeated or released.
    Key(KeyEvent),
    /// Text was typed or committed by an input method, while the window takes text input
    /// ([`Window::start_text_input`](crate::Window::start_text_input)).
    Text(TextEvent),
    /// The mouse moved.
    MouseMotion(MouseMotionEvent),
    /// A mouse button was pressed or released.
    MouseButton(MouseButtonEvent),
    /// The mouse wheel or a trackpad scrolled.
    MouseWheel(MouseWheelEvent),
    /// A finger touched, moved on or left a touch screen.
    Touch(TouchEvent),
    /// A gamepad connected, disconnected or changed state.
    Gamepad { timestamp: Timestamp, gamepad: GamepadId, event: GamepadEvent },
}

impl Event {
    /// When SDL observed the event.
    pub fn timestamp(&self) -> Timestamp {
        match self {
            Event::Quit { timestamp }
            | Event::Lifecycle { timestamp, .. }
            | Event::Window { timestamp, .. }
            | Event::Gamepad { timestamp, .. } => *timestamp,
            Event::Key(e) => e.timestamp,
            Event::Text(e) => e.timestamp,
            Event::MouseMotion(e) => e.timestamp,
            Event::MouseButton(e) => e.timestamp,
            Event::MouseWheel(e) => e.timestamp,
            Event::Touch(e) => e.timestamp,
        }
    }
}

/// OS lifecycle transitions (spec §6.5). Never sent on desktop.
///
/// On iOS they are delivered synchronously from inside the UIKit callback that raised
/// them, so work done in [`App::event`](crate::App::event) finishes before the OS moves
/// on. On Android they arrive on the SDL thread's next event pump. Each background /
/// foreground transition is delivered once per change (the platform drops the repeats
/// some SDL backends send); iOS also reports `DidEnterForeground` at launch.
///
/// # Rendering contract
///
/// - **All mobile platforms:** submit no GPU work between `WillEnterBackground` and
///   `DidEnterForeground`. iOS terminates apps that use the GPU in the background.
/// - **iOS:** the window's `CAMetalLayer` survives backgrounding; keep the surface.
/// - **Android:** the `ANativeWindow` behind the window is destroyed on every trip to
///   the background and replaced on the way back. Going to the background, two things
///   happen independently:
///   - The SDL thread delivers `WillEnterBackground` then `DidEnterBackground` on its
///     next event pump, then blocks inside the pump until the activity resumes, so
///     [`App::iterate`](crate::App::iterate) is not called while paused. Drop the wgpu
///     `Surface`, then its [`SurfaceHandle`](crate::SurfaceHandle), on
///     `WillEnterBackground`; the device and everything else stay alive.
///   - Android's UI thread runs `surfaceDestroyed`, where SDL clears the window's
///     `ANativeWindow` and releases its own reference, without waiting for the SDL
///     thread. This can land mid-frame, before `WillEnterBackground` is delivered. The
///     `SurfaceHandle` keeps its own reference, so the pointer wgpu holds stays valid,
///     but acquiring or presenting may now fail (wgpu reports the surface lost or
///     outdated): treat that as "skip this frame", never as fatal. From then on
///     [`Window::surface_handle`](crate::Window::surface_handle) returns
///     [`HandleError::Unavailable`](raw_window_handle::HandleError::Unavailable), so
///     don't try to rebuild the surface until the app is back in the foreground.
///
///   Coming back, `surfaceCreated` stores the new `ANativeWindow` before SDL lets its
///   thread resume (it waits for a ready surface and window focus), so `surface_handle()`
///   succeeds again by `WillEnterForeground`. Recreate the surface on
///   `DidEnterForeground` from a fresh `SurfaceHandle` (one kept from before the pause
///   names the destroyed window) and configure it from
///   [`Window::pixel_size`](crate::Window::pixel_size), which may have changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LifecycleEvent {
    /// The app is about to lose the foreground: stop GPU work, flush saves, pause
    /// audio and the world clock.
    WillEnterBackground,
    /// The app is in the background.
    DidEnterBackground,
    /// The app is about to return to the foreground.
    WillEnterForeground,
    /// The app is in the foreground again: recreate surfaces and resume.
    DidEnterForeground,
    /// A hint that memory is short: trim caches that are cheap to rebuild.
    ///
    /// On Android it is not a reliable emergency signal: SDL reports every
    /// `onTrimMemory` callback as `LowMemory`, including the one Android sends on every
    /// trip to the background. Don't drop caches that are expensive to rebuild on every
    /// pause. On iOS it is the system's memory warning, which is worth acting on.
    LowMemory,
    /// The OS is terminating the app. [`App::quit`](crate::App::quit) may never run.
    Terminating,
}

impl LifecycleEvent {
    /// Whether this is one of the four background / foreground transitions.
    pub fn is_background_transition(self) -> bool {
        matches!(
            self,
            Self::WillEnterBackground
                | Self::DidEnterBackground
                | Self::WillEnterForeground
                | Self::DidEnterForeground
        )
    }
}

/// Window state changes. Sizes are `(width, height)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WindowEvent {
    /// The window's size in window coordinates changed.
    Resized { width: u32, height: u32 },
    /// The window's drawable size in pixels changed: reconfigure the surface.
    PixelSizeChanged { width: u32, height: u32 },
    /// The user asked to close the window.
    CloseRequested,
    /// The window was minimized.
    Minimized,
    /// The window left the minimized or maximized state.
    Restored,
    /// The window gained keyboard focus.
    FocusGained,
    /// The window lost keyboard focus.
    FocusLost,
    /// The window's display scale changed; query
    /// [`Window::display_scale`](crate::Window::display_scale).
    DisplayScaleChanged,
    /// The window's safe area changed (for example after rotating between the two
    /// landscape orientations on a phone with a notch); query
    /// [`Window::safe_area`](crate::Window::safe_area).
    SafeAreaChanged,
}

/// A physical key position. Values are USB HID keyboard usage IDs (usage page 0x07),
/// as in SDL, which numbers a few extra media keys above 255.
///
/// Scancodes identify where a key is, independent of keyboard layout, so they are
/// what gameplay bindings use. The constants cover the keys the engine binds by
/// default; any other usage ID is equally valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Scancode(pub u16);

#[allow(missing_docs)]
impl Scancode {
    pub const A: Self = Self(4);
    pub const B: Self = Self(5);
    pub const C: Self = Self(6);
    pub const D: Self = Self(7);
    pub const E: Self = Self(8);
    pub const F: Self = Self(9);
    pub const G: Self = Self(10);
    pub const H: Self = Self(11);
    pub const I: Self = Self(12);
    pub const J: Self = Self(13);
    pub const K: Self = Self(14);
    pub const L: Self = Self(15);
    pub const M: Self = Self(16);
    pub const N: Self = Self(17);
    pub const O: Self = Self(18);
    pub const P: Self = Self(19);
    pub const Q: Self = Self(20);
    pub const R: Self = Self(21);
    pub const S: Self = Self(22);
    pub const T: Self = Self(23);
    pub const U: Self = Self(24);
    pub const V: Self = Self(25);
    pub const W: Self = Self(26);
    pub const X: Self = Self(27);
    pub const Y: Self = Self(28);
    pub const Z: Self = Self(29);
    pub const DIGIT_1: Self = Self(30);
    pub const DIGIT_2: Self = Self(31);
    pub const DIGIT_3: Self = Self(32);
    pub const DIGIT_4: Self = Self(33);
    pub const DIGIT_5: Self = Self(34);
    pub const DIGIT_6: Self = Self(35);
    pub const DIGIT_7: Self = Self(36);
    pub const DIGIT_8: Self = Self(37);
    pub const DIGIT_9: Self = Self(38);
    pub const DIGIT_0: Self = Self(39);
    pub const RETURN: Self = Self(40);
    pub const ESCAPE: Self = Self(41);
    pub const BACKSPACE: Self = Self(42);
    pub const TAB: Self = Self(43);
    pub const SPACE: Self = Self(44);
    pub const MINUS: Self = Self(45);
    pub const EQUALS: Self = Self(46);
    pub const LEFT_BRACKET: Self = Self(47);
    pub const RIGHT_BRACKET: Self = Self(48);
    pub const BACKSLASH: Self = Self(49);
    pub const SEMICOLON: Self = Self(51);
    pub const APOSTROPHE: Self = Self(52);
    pub const GRAVE: Self = Self(53);
    pub const COMMA: Self = Self(54);
    pub const PERIOD: Self = Self(55);
    pub const SLASH: Self = Self(56);
    pub const CAPS_LOCK: Self = Self(57);
    pub const F1: Self = Self(58);
    pub const F2: Self = Self(59);
    pub const F3: Self = Self(60);
    pub const F4: Self = Self(61);
    pub const F5: Self = Self(62);
    pub const F6: Self = Self(63);
    pub const F7: Self = Self(64);
    pub const F8: Self = Self(65);
    pub const F9: Self = Self(66);
    pub const F10: Self = Self(67);
    pub const F11: Self = Self(68);
    pub const F12: Self = Self(69);
    pub const PRINT_SCREEN: Self = Self(70);
    pub const SCROLL_LOCK: Self = Self(71);
    pub const PAUSE: Self = Self(72);
    pub const INSERT: Self = Self(73);
    pub const HOME: Self = Self(74);
    pub const PAGE_UP: Self = Self(75);
    pub const DELETE: Self = Self(76);
    pub const END: Self = Self(77);
    pub const PAGE_DOWN: Self = Self(78);
    pub const RIGHT: Self = Self(79);
    pub const LEFT: Self = Self(80);
    pub const DOWN: Self = Self(81);
    pub const UP: Self = Self(82);
    pub const LEFT_CTRL: Self = Self(224);
    pub const LEFT_SHIFT: Self = Self(225);
    pub const LEFT_ALT: Self = Self(226);
    pub const LEFT_GUI: Self = Self(227);
    pub const RIGHT_CTRL: Self = Self(228);
    pub const RIGHT_SHIFT: Self = Self(229);
    pub const RIGHT_ALT: Self = Self(230);
    pub const RIGHT_GUI: Self = Self(231);
}

/// The key's meaning under the current keyboard layout.
///
/// Use it for text-like shortcuts and for showing key names
/// ([`Platform::key_name`](crate::Platform::key_name)); bind gameplay to [`Scancode`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Keycode(pub u32);

impl Keycode {
    /// Keycodes with this bit set name non-character keys (arrows, F-keys, ...).
    const NON_CHARACTER: u32 = 1 << 30;

    /// The printable character this key produces in the current layout, if any.
    pub fn to_char(self) -> Option<char> {
        if self.0 & Self::NON_CHARACTER != 0 {
            return None;
        }
        char::from_u32(self.0).filter(|c| !c.is_control())
    }
}

/// Modifier keys held when a key event was generated (either side counts).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Command on Apple keyboards, the Windows key elsewhere.
    pub gui: bool,
}

impl Modifiers {
    fn from_sdl(mods: SDL_Keymod) -> Self {
        let has = |m: SDL_Keymod| mods.0 & m.0 != 0;
        Self {
            shift: has(SDL_Keymod::SHIFT),
            ctrl: has(SDL_Keymod::CTRL),
            alt: has(SDL_Keymod::ALT),
            gui: has(SDL_Keymod::GUI),
        }
    }
}

/// A keyboard key event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyEvent {
    pub timestamp: Timestamp,
    pub window: WindowId,
    pub scancode: Scancode,
    pub keycode: Keycode,
    pub modifiers: Modifiers,
    /// `true` for press and auto-repeat, `false` for release.
    pub down: bool,
    /// `true` if this is an OS auto-repeat of a held key.
    pub repeat: bool,
}

/// Typed text, in UTF-8: a character from the keyboard, or a string an input method (IME, the
/// on-screen keyboard) committed.
///
/// Key events still arrive for the keys behind it; use those for shortcuts and editing keys,
/// and this for the characters to insert. Text longer than [`TextEvent::CAPACITY`] bytes is
/// delivered as several events, in order, split between characters, so events stay `Copy` and
/// allocation-free.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextEvent {
    pub timestamp: Timestamp,
    pub window: WindowId,
    len: u8,
    bytes: [u8; TextEvent::CAPACITY],
}

impl TextEvent {
    /// The most UTF-8 bytes one event carries.
    pub const CAPACITY: usize = 32;

    /// The events that deliver `text`, each holding as many whole characters as fit. The
    /// platform delivers text this way; tests and input replays use it to synthesize text.
    pub fn split(timestamp: Timestamp, window: WindowId, text: &str) -> impl Iterator<Item = Self> + '_ {
        let mut rest = text;
        std::iter::from_fn(move || {
            if rest.is_empty() {
                return None;
            }
            let mut end = rest.len().min(Self::CAPACITY);
            while !rest.is_char_boundary(end) {
                end -= 1;
            }
            let (chunk, tail) = rest.split_at(end);
            rest = tail;
            let mut bytes = [0; Self::CAPACITY];
            bytes[..chunk.len()].copy_from_slice(chunk.as_bytes());
            Some(Self { timestamp, window, len: chunk.len() as u8, bytes })
        })
    }

    pub fn text(&self) -> &str {
        // `split` only stores whole characters of a `str`.
        std::str::from_utf8(&self.bytes[..usize::from(self.len)]).expect("TextEvent holds whole characters")
    }
}

impl fmt::Debug for TextEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextEvent")
            .field("timestamp", &self.timestamp)
            .field("window", &self.window)
            .field("text", &self.text())
            .finish()
    }
}

/// Mouse movement. Positions are in window coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MouseMotionEvent {
    pub timestamp: Timestamp,
    pub window: WindowId,
    pub x: f32,
    pub y: f32,
    /// Movement since the previous motion event; unaffected by the cursor hitting
    /// the screen edge in relative mode.
    pub dx: f32,
    pub dy: f32,
}

/// A mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    /// The first side ("back") button.
    X1,
    /// The second side ("forward") button.
    X2,
    /// Any further button, numbered from 6 as SDL reports it.
    Other(u8),
}

impl MouseButton {
    fn from_sdl(button: u8) -> Self {
        match button {
            1 => Self::Left,
            2 => Self::Middle,
            3 => Self::Right,
            4 => Self::X1,
            5 => Self::X2,
            n => Self::Other(n),
        }
    }
}

/// A mouse button press or release. Positions are in window coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MouseButtonEvent {
    pub timestamp: Timestamp,
    pub window: WindowId,
    pub button: MouseButton,
    pub down: bool,
    /// 1 for a single click, 2 for a double click, and so on.
    pub clicks: u8,
    pub x: f32,
    pub y: f32,
}

/// Scrolling. Positive `y` scrolls away from the user and positive `x` to the right,
/// regardless of the OS "natural scrolling" setting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MouseWheelEvent {
    pub timestamp: Timestamp,
    pub window: WindowId,
    pub x: f32,
    pub y: f32,
}

/// A source of [`TouchEvent`]s: a touch screen, or a pen acting as a finger.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TouchDeviceId(pub(crate) u64);

impl TouchDeviceId {
    /// A device for synthetic events (tests, input replays). Real ids come from SDL.
    pub const fn from_raw(id: u64) -> Self {
        Self(id)
    }

    /// Whether this is the pen (stylus) rather than a touch screen.
    pub fn is_pen(self) -> bool {
        self.0 == SDL_PEN_TOUCHID.0
    }
}

/// Identifies one finger for the duration of a touch, from [`TouchPhase::Down`] to
/// [`TouchPhase::Up`] or [`TouchPhase::Canceled`]. Unique across touch devices: the
/// pen's touch and a screen's first finger never compare equal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FingerId {
    pub(crate) device: TouchDeviceId,
    pub(crate) finger: u64,
}

impl FingerId {
    /// A finger for synthetic events (tests, input replays). Real ids come from SDL.
    pub const fn new(device: TouchDeviceId, finger: u64) -> Self {
        Self { device, finger }
    }

    /// The device this finger is on.
    pub fn device(self) -> TouchDeviceId {
        self.device
    }
}

/// Where a touch is in its lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TouchPhase {
    Down,
    Moved,
    Up,
    /// The OS took the touch away (for example, a system gesture began). Treat it as
    /// released without triggering the action a release would.
    Canceled,
}

/// A touch event.
///
/// Every physical input reaches the app exactly once, through one event family (the
/// platform turns off SDL's synthetic cross-device events):
///
/// | Device | Events |
/// |---|---|
/// | Mouse, trackpad | [`Event::MouseMotion`], [`Event::MouseButton`], [`Event::MouseWheel`] |
/// | Touch screen | [`Event::Touch`], one [`FingerId`] per finger |
/// | Pen / stylus (Apple Pencil, S Pen) | [`Event::Touch`] as a single finger on its own device ([`TouchDeviceId::is_pen`]); pressure is the pen's |
/// | Keyboard | [`Event::Key`] |
/// | Gamepad | [`Event::Gamepad`] (touchpads and motion sensors are not forwarded yet) |
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchEvent {
    pub timestamp: Timestamp,
    pub window: WindowId,
    pub finger: FingerId,
    pub phase: TouchPhase,
    /// Position normalized to the window, `0.0..=1.0` left to right.
    pub x: f32,
    /// Position normalized to the window, `0.0..=1.0` top to bottom.
    pub y: f32,
    /// Normalized pressure, `0.0..=1.0`; `1.0` on screens that don't sense pressure.
    pub pressure: f32,
}

/// Identifies a connected gamepad from [`GamepadEvent::Added`] until
/// [`GamepadEvent::Removed`]. Ids are never reused within a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GamepadId(pub(crate) u32);

/// Gamepad state changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GamepadEvent {
    /// A gamepad connected and has been opened; its input events follow.
    Added {
        /// The controller SDL identified, for button glyphs.
        kind: GamepadType,
    },
    /// The gamepad disconnected.
    Removed,
    Button {
        button: GamepadButton,
        down: bool,
    },
    /// Sticks report `-1.0..=1.0` (positive is right / down); triggers `0.0..=1.0`.
    Axis {
        axis: GamepadAxis,
        value: f32,
    },
}

/// What controller a gamepad is, as SDL identifies it from its USB IDs and mapping. Prompt
/// glyphs follow it (spec §5.18); buttons are always named by position ([`GamepadButton`]),
/// whatever their labels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GamepadType {
    /// SDL can't tell what this controller is.
    Unknown,
    /// A controller with a standard layout but no recognized family.
    Standard,
    Xbox360,
    XboxOne,
    PlayStation3,
    PlayStation4,
    PlayStation5,
    SwitchPro,
    JoyConLeft,
    JoyConRight,
    /// Two Joy-Cons acting as one controller.
    JoyConPair,
    GameCube,
    /// Valve's Steam Controller and Steam Deck.
    Steam,
}

impl GamepadType {
    pub(crate) fn from_sdl(kind: SDL_GamepadType) -> Self {
        match kind {
            SDL_GamepadType::STANDARD => Self::Standard,
            SDL_GamepadType::XBOX360 => Self::Xbox360,
            SDL_GamepadType::XBOXONE => Self::XboxOne,
            SDL_GamepadType::PS3 => Self::PlayStation3,
            SDL_GamepadType::PS4 => Self::PlayStation4,
            SDL_GamepadType::PS5 => Self::PlayStation5,
            SDL_GamepadType::NINTENDO_SWITCH_PRO => Self::SwitchPro,
            SDL_GamepadType::NINTENDO_SWITCH_JOYCON_LEFT => Self::JoyConLeft,
            SDL_GamepadType::NINTENDO_SWITCH_JOYCON_RIGHT => Self::JoyConRight,
            SDL_GamepadType::NINTENDO_SWITCH_JOYCON_PAIR => Self::JoyConPair,
            SDL_GamepadType::GAMECUBE => Self::GameCube,
            SDL_GamepadType::STEAM => Self::Steam,
            _ => Self::Unknown,
        }
    }
}

/// A gamepad button, named by position (Xbox layout) rather than label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GamepadButton {
    /// Bottom face button (Xbox A, PlayStation Cross).
    South,
    /// Right face button (Xbox B, PlayStation Circle).
    East,
    /// Left face button (Xbox X, PlayStation Square).
    West,
    /// Top face button (Xbox Y, PlayStation Triangle).
    North,
    Back,
    Guide,
    Start,
    LeftStick,
    RightStick,
    LeftShoulder,
    RightShoulder,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    /// Share / capture / microphone button, depending on the controller.
    Misc1,
    RightPaddle1,
    LeftPaddle1,
    RightPaddle2,
    LeftPaddle2,
    /// PlayStation touchpad click.
    Touchpad,
    Misc2,
    Misc3,
    Misc4,
    Misc5,
    Misc6,
}

impl GamepadButton {
    fn from_sdl(button: SDL_GamepadButton) -> Option<Self> {
        Some(match button {
            SDL_GamepadButton::SOUTH => Self::South,
            SDL_GamepadButton::EAST => Self::East,
            SDL_GamepadButton::WEST => Self::West,
            SDL_GamepadButton::NORTH => Self::North,
            SDL_GamepadButton::BACK => Self::Back,
            SDL_GamepadButton::GUIDE => Self::Guide,
            SDL_GamepadButton::START => Self::Start,
            SDL_GamepadButton::LEFT_STICK => Self::LeftStick,
            SDL_GamepadButton::RIGHT_STICK => Self::RightStick,
            SDL_GamepadButton::LEFT_SHOULDER => Self::LeftShoulder,
            SDL_GamepadButton::RIGHT_SHOULDER => Self::RightShoulder,
            SDL_GamepadButton::DPAD_UP => Self::DpadUp,
            SDL_GamepadButton::DPAD_DOWN => Self::DpadDown,
            SDL_GamepadButton::DPAD_LEFT => Self::DpadLeft,
            SDL_GamepadButton::DPAD_RIGHT => Self::DpadRight,
            SDL_GamepadButton::MISC1 => Self::Misc1,
            SDL_GamepadButton::RIGHT_PADDLE1 => Self::RightPaddle1,
            SDL_GamepadButton::LEFT_PADDLE1 => Self::LeftPaddle1,
            SDL_GamepadButton::RIGHT_PADDLE2 => Self::RightPaddle2,
            SDL_GamepadButton::LEFT_PADDLE2 => Self::LeftPaddle2,
            SDL_GamepadButton::TOUCHPAD => Self::Touchpad,
            SDL_GamepadButton::MISC2 => Self::Misc2,
            SDL_GamepadButton::MISC3 => Self::Misc3,
            SDL_GamepadButton::MISC4 => Self::Misc4,
            SDL_GamepadButton::MISC5 => Self::Misc5,
            SDL_GamepadButton::MISC6 => Self::Misc6,
            _ => return None,
        })
    }
}

/// A gamepad analog axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GamepadAxis {
    LeftX,
    LeftY,
    RightX,
    RightY,
    LeftTrigger,
    RightTrigger,
}

impl GamepadAxis {
    fn from_sdl(axis: SDL_GamepadAxis) -> Option<Self> {
        Some(match axis {
            SDL_GamepadAxis::LEFTX => Self::LeftX,
            SDL_GamepadAxis::LEFTY => Self::LeftY,
            SDL_GamepadAxis::RIGHTX => Self::RightX,
            SDL_GamepadAxis::RIGHTY => Self::RightY,
            SDL_GamepadAxis::LEFT_TRIGGER => Self::LeftTrigger,
            SDL_GamepadAxis::RIGHT_TRIGGER => Self::RightTrigger,
            _ => return None,
        })
    }
}

/// Maps SDL's `-32768..=32767` axis range to `-1.0..=1.0` (triggers only report the
/// non-negative half, giving `0.0..=1.0`).
fn normalize_axis(value: i16) -> f32 {
    (f32::from(value) / f32::from(i16::MAX)).max(-1.0)
}

fn size(width: i32, height: i32) -> (u32, u32) {
    (u32::try_from(width).unwrap_or(0), u32::try_from(height).unwrap_or(0))
}

fn window_event(tag: SDL_EventType, data1: i32, data2: i32) -> Option<WindowEvent> {
    Some(match tag {
        SDL_EventType::WINDOW_RESIZED => {
            let (width, height) = size(data1, data2);
            WindowEvent::Resized { width, height }
        }
        SDL_EventType::WINDOW_PIXEL_SIZE_CHANGED => {
            let (width, height) = size(data1, data2);
            WindowEvent::PixelSizeChanged { width, height }
        }
        SDL_EventType::WINDOW_CLOSE_REQUESTED => WindowEvent::CloseRequested,
        SDL_EventType::WINDOW_MINIMIZED => WindowEvent::Minimized,
        SDL_EventType::WINDOW_RESTORED => WindowEvent::Restored,
        SDL_EventType::WINDOW_FOCUS_GAINED => WindowEvent::FocusGained,
        SDL_EventType::WINDOW_FOCUS_LOST => WindowEvent::FocusLost,
        SDL_EventType::WINDOW_DISPLAY_SCALE_CHANGED => WindowEvent::DisplayScaleChanged,
        SDL_EventType::WINDOW_SAFE_AREA_CHANGED => WindowEvent::SafeAreaChanged,
        _ => return None,
    })
}

/// The text of an SDL text-input event, with its timestamp and window; `None` for any other
/// event. Kept apart from [`translate`] because one SDL event can become several
/// [`TextEvent`]s.
///
/// IME composition (`SDL_EVENT_TEXT_EDITING`, the text being composed before it is committed)
/// is not forwarded yet: committed text arrives here either way.
pub(crate) fn text_input(raw: &SDL_Event) -> Option<(Timestamp, WindowId, std::borrow::Cow<'_, str>)> {
    // SAFETY: every SDL event variant begins with the common header, so the type is initialized.
    if SDL_EventType(unsafe { raw.r#type }) != SDL_EventType::TEXT_INPUT {
        return None;
    }
    // SAFETY: the tag says this is a text-input event.
    let e = unsafe { &raw.text };
    let text = if e.text.is_null() {
        std::borrow::Cow::Borrowed("")
    } else {
        // SAFETY: SDL hands out a NUL-terminated UTF-8 string that stays valid while the event is
        // dispatched, which is as long as `raw` is borrowed here. Lossy conversion only guards
        // against a backend sending invalid UTF-8; it borrows otherwise.
        unsafe { CStr::from_ptr(e.text) }.to_string_lossy()
    };
    Some((Timestamp(e.timestamp), WindowId(e.windowID.0), text))
}

/// Translates an SDL event, or returns `None` for event kinds the engine doesn't use. Text
/// input is translated by [`text_input`].
pub(crate) fn translate(raw: &SDL_Event) -> Option<Event> {
    // SAFETY: every SDL event variant begins with the common header (type, reserved,
    // timestamp), so these two fields are initialized whatever the variant.
    let (tag, timestamp) = unsafe { (SDL_EventType(raw.r#type), Timestamp(raw.common.timestamp)) };
    let lifecycle = |event| Some(Event::Lifecycle { timestamp, event });

    match tag {
        SDL_EventType::QUIT => Some(Event::Quit { timestamp }),
        SDL_EventType::TERMINATING => lifecycle(LifecycleEvent::Terminating),
        SDL_EventType::LOW_MEMORY => lifecycle(LifecycleEvent::LowMemory),
        SDL_EventType::WILL_ENTER_BACKGROUND => lifecycle(LifecycleEvent::WillEnterBackground),
        SDL_EventType::DID_ENTER_BACKGROUND => lifecycle(LifecycleEvent::DidEnterBackground),
        SDL_EventType::WILL_ENTER_FOREGROUND => lifecycle(LifecycleEvent::WillEnterForeground),
        SDL_EventType::DID_ENTER_FOREGROUND => lifecycle(LifecycleEvent::DidEnterForeground),

        _ if (SDL_EventType::WINDOW_FIRST.0..=SDL_EventType::WINDOW_LAST.0).contains(&tag.0) => {
            // SAFETY: the tag is in SDL's window-event range, so `window` is the active field.
            let e = unsafe { &raw.window };
            let event = window_event(tag, e.data1, e.data2)?;
            Some(Event::Window { timestamp, window: WindowId(e.windowID.0), event })
        }

        SDL_EventType::KEY_DOWN | SDL_EventType::KEY_UP => {
            // SAFETY: the tag says this is a keyboard event.
            let e = unsafe { &raw.key };
            Some(Event::Key(KeyEvent {
                timestamp,
                window: WindowId(e.windowID.0),
                scancode: Scancode(u16::try_from(e.scancode.0).ok()?),
                keycode: Keycode(e.key.0),
                modifiers: Modifiers::from_sdl(e.r#mod),
                down: e.down,
                repeat: e.repeat,
            }))
        }

        SDL_EventType::MOUSE_MOTION => {
            // SAFETY: the tag says this is a mouse-motion event.
            let e = unsafe { &raw.motion };
            Some(Event::MouseMotion(MouseMotionEvent {
                timestamp,
                window: WindowId(e.windowID.0),
                x: e.x,
                y: e.y,
                dx: e.xrel,
                dy: e.yrel,
            }))
        }
        SDL_EventType::MOUSE_BUTTON_DOWN | SDL_EventType::MOUSE_BUTTON_UP => {
            // SAFETY: the tag says this is a mouse-button event.
            let e = unsafe { &raw.button };
            Some(Event::MouseButton(MouseButtonEvent {
                timestamp,
                window: WindowId(e.windowID.0),
                button: MouseButton::from_sdl(e.button),
                down: e.down,
                clicks: e.clicks,
                x: e.x,
                y: e.y,
            }))
        }
        SDL_EventType::MOUSE_WHEEL => {
            // SAFETY: the tag says this is a mouse-wheel event.
            let e = unsafe { &raw.wheel };
            let sign = if e.direction == SDL_MOUSEWHEEL_FLIPPED { -1.0 } else { 1.0 };
            Some(Event::MouseWheel(MouseWheelEvent {
                timestamp,
                window: WindowId(e.windowID.0),
                x: e.x * sign,
                y: e.y * sign,
            }))
        }

        SDL_EventType::FINGER_DOWN
        | SDL_EventType::FINGER_MOTION
        | SDL_EventType::FINGER_UP
        | SDL_EventType::FINGER_CANCELED => {
            // SAFETY: the tag says this is a touch-finger event.
            let e = unsafe { &raw.tfinger };
            let phase = match tag {
                SDL_EventType::FINGER_DOWN => TouchPhase::Down,
                SDL_EventType::FINGER_MOTION => TouchPhase::Moved,
                SDL_EventType::FINGER_UP => TouchPhase::Up,
                _ => TouchPhase::Canceled,
            };
            Some(Event::Touch(TouchEvent {
                timestamp,
                window: WindowId(e.windowID.0),
                finger: FingerId { device: TouchDeviceId(e.touchID.0), finger: e.fingerID.0 },
                phase,
                x: e.x,
                y: e.y,
                pressure: e.pressure,
            }))
        }

        SDL_EventType::GAMEPAD_ADDED | SDL_EventType::GAMEPAD_REMOVED => {
            // SAFETY: the tag says this is a gamepad-device event.
            let e = unsafe { &raw.gdevice };
            // SDL knows the type once the gamepad is open, so `Platform::translate_event`, which
            // opens it, fills in the kind.
            let event = if tag == SDL_EventType::GAMEPAD_ADDED {
                GamepadEvent::Added { kind: GamepadType::Unknown }
            } else {
                GamepadEvent::Removed
            };
            Some(Event::Gamepad { timestamp, gamepad: GamepadId(e.which.0), event })
        }
        SDL_EventType::GAMEPAD_BUTTON_DOWN | SDL_EventType::GAMEPAD_BUTTON_UP => {
            // SAFETY: the tag says this is a gamepad-button event.
            let e = unsafe { &raw.gbutton };
            let button = GamepadButton::from_sdl(SDL_GamepadButton(e.button.into()))?;
            Some(Event::Gamepad {
                timestamp,
                gamepad: GamepadId(e.which.0),
                event: GamepadEvent::Button { button, down: e.down },
            })
        }
        SDL_EventType::GAMEPAD_AXIS_MOTION => {
            // SAFETY: the tag says this is a gamepad-axis event.
            let e = unsafe { &raw.gaxis };
            let axis = GamepadAxis::from_sdl(SDL_GamepadAxis(e.axis.into()))?;
            Some(Event::Gamepad {
                timestamp,
                gamepad: GamepadId(e.which.0),
                event: GamepadEvent::Axis { axis, value: normalize_axis(e.value) },
            })
        }

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use sdl3_sys::events::{
        SDL_GamepadAxisEvent, SDL_GamepadButtonEvent, SDL_GamepadDeviceEvent, SDL_KeyboardEvent,
        SDL_MouseWheelEvent, SDL_TextInputEvent, SDL_TouchFingerEvent, SDL_WindowEvent,
    };
    use sdl3_sys::joystick::SDL_JoystickID;
    use sdl3_sys::keycode::{SDL_KMOD_LCTRL, SDL_KMOD_RSHIFT, SDLK_A, SDLK_UP};
    use sdl3_sys::scancode::*;
    use sdl3_sys::touch::{SDL_FingerID, SDL_TouchID};
    use sdl3_sys::video::SDL_WindowID;

    use super::*;

    fn event_with(build: impl FnOnce(&mut SDL_Event)) -> SDL_Event {
        let mut raw = SDL_Event::default();
        build(&mut raw);
        raw
    }

    #[test]
    fn scancode_constants_match_sdl() {
        let pairs = [
            (Scancode::A, SDL_SCANCODE_A),
            (Scancode::Z, SDL_SCANCODE_Z),
            (Scancode::DIGIT_1, SDL_SCANCODE_1),
            (Scancode::DIGIT_0, SDL_SCANCODE_0),
            (Scancode::RETURN, SDL_SCANCODE_RETURN),
            (Scancode::ESCAPE, SDL_SCANCODE_ESCAPE),
            (Scancode::SPACE, SDL_SCANCODE_SPACE),
            (Scancode::SEMICOLON, SDL_SCANCODE_SEMICOLON),
            (Scancode::GRAVE, SDL_SCANCODE_GRAVE),
            (Scancode::SLASH, SDL_SCANCODE_SLASH),
            (Scancode::F1, SDL_SCANCODE_F1),
            (Scancode::F12, SDL_SCANCODE_F12),
            (Scancode::PRINT_SCREEN, SDL_SCANCODE_PRINTSCREEN),
            (Scancode::PAGE_DOWN, SDL_SCANCODE_PAGEDOWN),
            (Scancode::RIGHT, SDL_SCANCODE_RIGHT),
            (Scancode::UP, SDL_SCANCODE_UP),
            (Scancode::LEFT_CTRL, SDL_SCANCODE_LCTRL),
            (Scancode::RIGHT_GUI, SDL_SCANCODE_RGUI),
        ];
        for (ours, sdl) in pairs {
            assert_eq!(i32::from(ours.0), sdl.0, "{ours:?}");
        }
    }

    #[test]
    fn keycode_chars() {
        assert_eq!(Keycode(SDLK_A.0).to_char(), Some('a'));
        assert_eq!(Keycode(SDLK_UP.0).to_char(), None);
        assert_eq!(Keycode(u32::from('\r')).to_char(), None);
    }

    #[test]
    fn key_event_keeps_timestamp_and_fields() {
        let raw = event_with(|e| {
            e.key = SDL_KeyboardEvent {
                r#type: SDL_EventType::KEY_DOWN,
                timestamp: 1_234_567,
                windowID: SDL_WindowID(3),
                scancode: SDL_SCANCODE_W,
                key: SDLK_A,
                r#mod: SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_RSHIFT.0),
                down: true,
                repeat: true,
                ..Default::default()
            }
        });
        let Some(Event::Key(key)) = translate(&raw) else { panic!("expected a key event") };
        assert_eq!(key.timestamp, Timestamp::from_nanos(1_234_567));
        assert_eq!(key.window, WindowId(3));
        assert_eq!(key.scancode, Scancode::W);
        assert_eq!(key.keycode.to_char(), Some('a'));
        assert_eq!(key.modifiers, Modifiers { shift: true, ctrl: true, alt: false, gui: false });
        assert!(key.down && key.repeat);
        assert_eq!(Event::Key(key).timestamp().as_nanos(), 1_234_567);
    }

    #[test]
    fn window_events() {
        let window = |tag, data1, data2| {
            event_with(|e| {
                e.window = SDL_WindowEvent {
                    r#type: tag,
                    timestamp: 9,
                    windowID: SDL_WindowID(1),
                    data1,
                    data2,
                    ..Default::default()
                }
            })
        };
        assert_eq!(
            translate(&window(SDL_EventType::WINDOW_PIXEL_SIZE_CHANGED, 2560, 1440)),
            Some(Event::Window {
                timestamp: Timestamp::from_nanos(9),
                window: WindowId(1),
                event: WindowEvent::PixelSizeChanged { width: 2560, height: 1440 },
            })
        );
        assert!(matches!(
            translate(&window(SDL_EventType::WINDOW_RESIZED, -5, 10)),
            Some(Event::Window { event: WindowEvent::Resized { width: 0, height: 10 }, .. })
        ));
        assert!(matches!(
            translate(&window(SDL_EventType::WINDOW_CLOSE_REQUESTED, 0, 0)),
            Some(Event::Window { event: WindowEvent::CloseRequested, .. })
        ));
        // Window events the engine doesn't model are dropped, not mistranslated.
        assert_eq!(translate(&window(SDL_EventType::WINDOW_MOVED, 10, 10)), None);
    }

    #[test]
    fn lifecycle_events() {
        let cases = [
            (SDL_EventType::WILL_ENTER_BACKGROUND, LifecycleEvent::WillEnterBackground),
            (SDL_EventType::DID_ENTER_BACKGROUND, LifecycleEvent::DidEnterBackground),
            (SDL_EventType::WILL_ENTER_FOREGROUND, LifecycleEvent::WillEnterForeground),
            (SDL_EventType::DID_ENTER_FOREGROUND, LifecycleEvent::DidEnterForeground),
            (SDL_EventType::LOW_MEMORY, LifecycleEvent::LowMemory),
            (SDL_EventType::TERMINATING, LifecycleEvent::Terminating),
        ];
        for (tag, expected) in cases {
            let raw = event_with(|e| {
                e.common.r#type = tag.0;
                e.common.timestamp = 42;
            });
            assert_eq!(
                translate(&raw),
                Some(Event::Lifecycle { timestamp: Timestamp::from_nanos(42), event: expected })
            );
        }
    }

    #[test]
    fn flipped_wheel_is_normalized() {
        let raw = event_with(|e| {
            e.wheel = SDL_MouseWheelEvent {
                r#type: SDL_EventType::MOUSE_WHEEL,
                x: 0.5,
                y: 2.0,
                direction: SDL_MOUSEWHEEL_FLIPPED,
                ..Default::default()
            }
        });
        let Some(Event::MouseWheel(wheel)) = translate(&raw) else { panic!("expected a wheel event") };
        assert_eq!((wheel.x, wheel.y), (-0.5, -2.0));
    }

    #[test]
    fn touch_events() {
        let touch = |device: SDL_TouchID, finger: u64| {
            let raw = event_with(|e| {
                e.tfinger = SDL_TouchFingerEvent {
                    r#type: SDL_EventType::FINGER_CANCELED,
                    timestamp: 5,
                    touchID: device,
                    fingerID: SDL_FingerID(finger),
                    x: 0.25,
                    y: 0.75,
                    pressure: 0.5,
                    ..Default::default()
                }
            });
            match translate(&raw) {
                Some(Event::Touch(touch)) => touch,
                other => panic!("expected a touch event, got {other:?}"),
            }
        };

        let screen = touch(SDL_TouchID(3), 1);
        assert_eq!(screen.phase, TouchPhase::Canceled);
        assert_eq!((screen.x, screen.y, screen.pressure), (0.25, 0.75, 0.5));
        assert!(!screen.finger.device().is_pen());

        // SDL's synthetic pen touch is always finger 1, like many screens' first finger.
        let pen = touch(SDL_PEN_TOUCHID, 1);
        assert!(pen.finger.device().is_pen());
        assert_ne!(pen.finger, screen.finger);
        assert_eq!(touch(SDL_TouchID(3), 1).finger, screen.finger);
    }

    #[test]
    fn gamepad_events() {
        let added = event_with(|e| {
            e.gdevice = SDL_GamepadDeviceEvent {
                r#type: SDL_EventType::GAMEPAD_ADDED,
                which: SDL_JoystickID(8),
                ..Default::default()
            }
        });
        assert!(matches!(
            translate(&added),
            Some(Event::Gamepad { gamepad: GamepadId(8), event: GamepadEvent::Added { .. }, .. })
        ));
        assert_eq!(GamepadType::from_sdl(SDL_GamepadType::PS5), GamepadType::PlayStation5);
        assert_eq!(GamepadType::from_sdl(SDL_GamepadType::COUNT), GamepadType::Unknown);

        let button = event_with(|e| {
            e.gbutton = SDL_GamepadButtonEvent {
                r#type: SDL_EventType::GAMEPAD_BUTTON_DOWN,
                which: SDL_JoystickID(8),
                button: SDL_GamepadButton::EAST.0 as u8,
                down: true,
                ..Default::default()
            }
        });
        assert!(matches!(
            translate(&button),
            Some(Event::Gamepad {
                event: GamepadEvent::Button { button: GamepadButton::East, down: true },
                ..
            })
        ));

        let axis = |axis: SDL_GamepadAxis, value| {
            let raw = event_with(|e| {
                e.gaxis = SDL_GamepadAxisEvent {
                    r#type: SDL_EventType::GAMEPAD_AXIS_MOTION,
                    axis: axis.0 as u8,
                    value,
                    ..Default::default()
                }
            });
            match translate(&raw) {
                Some(Event::Gamepad { event: GamepadEvent::Axis { value, .. }, .. }) => value,
                other => panic!("expected an axis event, got {other:?}"),
            }
        };
        assert_eq!(axis(SDL_GamepadAxis::LEFTX, i16::MIN), -1.0);
        assert_eq!(axis(SDL_GamepadAxis::LEFTY, i16::MAX), 1.0);
        assert_eq!(axis(SDL_GamepadAxis::RIGHT_TRIGGER, 0), 0.0);
    }

    #[test]
    fn text_input_splits_between_characters() {
        let text = c"héllo, 世界! A longer IME commit than one event holds.";
        let raw = event_with(|e| {
            e.text = SDL_TextInputEvent {
                r#type: SDL_EventType::TEXT_INPUT,
                timestamp: 77,
                windowID: SDL_WindowID(2),
                text: text.as_ptr(),
                ..Default::default()
            }
        });
        assert_eq!(translate(&raw), None, "text goes through `text_input`");
        let (timestamp, window, text) = text_input(&raw).expect("a text event");
        assert_eq!((timestamp, window), (Timestamp::from_nanos(77), WindowId(2)));
        let chunks: Vec<TextEvent> = TextEvent::split(timestamp, window, &text).collect();
        assert!(chunks.len() > 1 && chunks.iter().all(|chunk| chunk.text().len() <= TextEvent::CAPACITY));
        assert_eq!(chunks.iter().map(TextEvent::text).collect::<String>(), text);
        assert!(chunks.iter().all(|chunk| chunk.timestamp == timestamp && chunk.window == window));

        // A multi-byte character that would straddle the capacity starts the next event.
        let edge = format!("{}é", "a".repeat(TextEvent::CAPACITY - 1));
        let lengths: Vec<usize> =
            TextEvent::split(timestamp, window, &edge).map(|chunk| chunk.text().len()).collect();
        assert_eq!(lengths, [TextEvent::CAPACITY - 1, 2]);
        assert_eq!(TextEvent::split(timestamp, window, "").count(), 0);
        assert_eq!(text_input(&event_with(|e| e.common.r#type = SDL_EventType::KEY_DOWN.0)), None);
    }

    #[test]
    fn unmodeled_events_are_dropped() {
        let raw = event_with(|e| e.common.r#type = SDL_EventType::CLIPBOARD_UPDATE.0);
        assert_eq!(translate(&raw), None);
    }

    #[test]
    fn timestamps_saturate() {
        let early = Timestamp::from_nanos(10);
        let late = Timestamp::from_nanos(25);
        assert_eq!(late.saturating_duration_since(early), Duration::from_nanos(15));
        assert_eq!(early.saturating_duration_since(late), Duration::ZERO);
    }
}
