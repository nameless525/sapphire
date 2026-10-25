//! The binding vocabulary: every physical input a binding can read ([`Source`]).
//!
//! These names are a data format, stored in bindings documents and, from M3, in user settings,
//! so this crate owns them rather than reusing `sapphire_platform`'s types, which follow SDL.

use bevy_reflect::Reflect;
use sapphire_platform::{GamepadAxis, GamepadButton, Scancode};
use serde::{Deserialize, Serialize};

/// A physical input a binding reads: Enhanced Input's "key".
///
/// Every source yields a 2D value; one-dimensional sources fill `x` and leave `y` at zero.
/// **Vertical values are positive up** for every device (toward the top of the screen for
/// pointers, away from the player on a stick), although SDL reports screen-down and stick-down
/// as positive.
///
/// | Source | Value | Updates |
/// |---|---|---|
/// | `Key`, `Mouse`, `Pad` | 1 while held | on each change |
/// | `PadAxis` | stick axes −1..=1, triggers 0..=1 | on each change |
/// | `PadStick` | both axes of a stick | on each change |
/// | `MouseMotion` | window points moved since the last update | per update |
/// | `MouseWheel`, `MouseWheelHorizontal` | notches since the last update: + away from the user, + right | per update |
/// | `Touch(Press)` | 1 while a lone finger rests without dragging | on each change |
/// | `Touch(Drag)` | window points a lone finger moved since the last update | per update |
/// | `Touch(Pinch)` | the natural log of how much two fingers spread since the last update | per update |
/// | `Touch(Swipe(direction))` | 1 for an instant when a swipe ends | on each swipe |
/// | `Virtual(name)` | what the on-screen control injects | on each change |
///
/// With several gamepads connected they act as one: a button is held if any pad holds it, and
/// an axis or stick reads the pad pushing it furthest.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::input"]
pub enum Source {
    Key(Key),
    Mouse(MouseButton),
    MouseMotion,
    MouseWheel,
    MouseWheelHorizontal,
    Pad(PadButton),
    PadAxis(PadAxis),
    PadStick(Stick),
    Touch(Gesture),
    /// An on-screen control by name, driven through
    /// [`InputSystem::inject`](crate::InputSystem::inject).
    Virtual(String),
}

macro_rules! keys {
    ($($key:ident = $scancode:ident),* $(,)?) => {
        /// A keyboard key by position (a USB HID usage), named as on a US layout. Bindings use
        /// positions, so WASD stays WASD on AZERTY keyboards.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
        #[type_path = "sapphire::input"]
        pub enum Key {
            $($key,)*
            /// Any other key, by its USB HID usage ID.
            Hid(u16),
        }

        impl Key {
            /// The physical key this names.
            pub fn scancode(self) -> Scancode {
                match self {
                    $(Self::$key => Scancode::$scancode,)*
                    Self::Hid(usage) => Scancode(usage),
                }
            }

            /// The key at `scancode`, by name where it has one.
            pub fn from_scancode(scancode: Scancode) -> Self {
                match scancode {
                    $(Scancode::$scancode => Self::$key,)*
                    Scancode(usage) => Self::Hid(usage),
                }
            }
        }
    };
}

keys! {
    A = A, B = B, C = C, D = D, E = E, F = F, G = G, H = H, I = I, J = J, K = K, L = L, M = M,
    N = N, O = O, P = P, Q = Q, R = R, S = S, T = T, U = U, V = V, W = W, X = X, Y = Y, Z = Z,
    Digit1 = DIGIT_1, Digit2 = DIGIT_2, Digit3 = DIGIT_3, Digit4 = DIGIT_4, Digit5 = DIGIT_5,
    Digit6 = DIGIT_6, Digit7 = DIGIT_7, Digit8 = DIGIT_8, Digit9 = DIGIT_9, Digit0 = DIGIT_0,
    Return = RETURN, Escape = ESCAPE, Backspace = BACKSPACE, Tab = TAB, Space = SPACE,
    Minus = MINUS, Equals = EQUALS, LeftBracket = LEFT_BRACKET, RightBracket = RIGHT_BRACKET,
    Backslash = BACKSLASH, Semicolon = SEMICOLON, Apostrophe = APOSTROPHE, Grave = GRAVE,
    Comma = COMMA, Period = PERIOD, Slash = SLASH, CapsLock = CAPS_LOCK,
    F1 = F1, F2 = F2, F3 = F3, F4 = F4, F5 = F5, F6 = F6, F7 = F7, F8 = F8, F9 = F9, F10 = F10,
    F11 = F11, F12 = F12,
    PrintScreen = PRINT_SCREEN, ScrollLock = SCROLL_LOCK, Pause = PAUSE, Insert = INSERT,
    Home = HOME, PageUp = PAGE_UP, Delete = DELETE, End = END, PageDown = PAGE_DOWN,
    Right = RIGHT, Left = LEFT, Down = DOWN, Up = UP,
    LeftCtrl = LEFT_CTRL, LeftShift = LEFT_SHIFT, LeftAlt = LEFT_ALT, LeftGui = LEFT_GUI,
    RightCtrl = RIGHT_CTRL, RightShift = RIGHT_SHIFT, RightAlt = RIGHT_ALT, RightGui = RIGHT_GUI,
}

/// A mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::input"]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    /// The first side button.
    Back,
    /// The second side button.
    Forward,
    /// A further button, numbered from 6 as SDL reports it.
    Other(u8),
}

impl MouseButton {
    pub fn from_platform(button: sapphire_platform::MouseButton) -> Self {
        use sapphire_platform::MouseButton as P;
        match button {
            P::Left => Self::Left,
            P::Middle => Self::Middle,
            P::Right => Self::Right,
            P::X1 => Self::Back,
            P::X2 => Self::Forward,
            P::Other(n) => Self::Other(n),
        }
    }

    /// Bit in the mouse-button state; buttons past 31 aren't tracked.
    pub(crate) fn bit(self) -> Option<u32> {
        match self {
            Self::Left => Some(0),
            Self::Middle => Some(1),
            Self::Right => Some(2),
            Self::Back => Some(3),
            Self::Forward => Some(4),
            Self::Other(n) => (5..32).contains(&n).then_some(u32::from(n)),
        }
    }
}

macro_rules! pad_buttons {
    ($($(#[$doc:meta])* $button:ident),* $(,)?) => {
        /// A gamepad button, named by position (Xbox layout) rather than label, as in
        /// `sapphire_platform`.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
        #[type_path = "sapphire::input"]
        pub enum PadButton {
            $($(#[$doc])* $button,)*
        }

        impl PadButton {
            /// The platform's button, or `None` for one newer than this vocabulary.
            pub fn from_platform(button: GamepadButton) -> Option<Self> {
                match button {
                    $(GamepadButton::$button => Some(Self::$button),)*
                    _ => None,
                }
            }

            /// Bit in a pad's button state.
            pub(crate) fn bit(self) -> u32 {
                self as u32
            }
        }
    };
}

pad_buttons! {
    /// Bottom face button (Xbox A, PlayStation Cross).
    South,
    /// Right face button (Xbox B, PlayStation Circle).
    East,
    /// Left face button (Xbox X, PlayStation Square).
    West,
    /// Top face button (Xbox Y, PlayStation Triangle).
    North,
    Back, Guide, Start, LeftStick, RightStick, LeftShoulder, RightShoulder,
    DpadUp, DpadDown, DpadLeft, DpadRight,
    Misc1, RightPaddle1, LeftPaddle1, RightPaddle2, LeftPaddle2, Touchpad,
    Misc2, Misc3, Misc4, Misc5, Misc6,
}

/// One analog gamepad axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::input"]
pub enum PadAxis {
    LeftX,
    LeftY,
    RightX,
    RightY,
    LeftTrigger,
    RightTrigger,
}

impl PadAxis {
    pub fn from_platform(axis: GamepadAxis) -> Self {
        match axis {
            GamepadAxis::LeftX => Self::LeftX,
            GamepadAxis::LeftY => Self::LeftY,
            GamepadAxis::RightX => Self::RightX,
            GamepadAxis::RightY => Self::RightY,
            GamepadAxis::LeftTrigger => Self::LeftTrigger,
            GamepadAxis::RightTrigger => Self::RightTrigger,
        }
    }

    pub(crate) fn index(self) -> usize {
        self as usize
    }

    /// SDL reports these positive down; sources report them positive up.
    pub(crate) fn is_vertical(self) -> bool {
        matches!(self, Self::LeftY | Self::RightY)
    }
}

/// Both axes of a gamepad stick.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::input"]
pub enum Stick {
    Left,
    Right,
}

impl Stick {
    pub(crate) fn axes(self) -> (PadAxis, PadAxis) {
        match self {
            Self::Left => (PadAxis::LeftX, PadAxis::LeftY),
            Self::Right => (PadAxis::RightX, PadAxis::RightY),
        }
    }
}

/// A touch gesture the crate recognizes (see [`crate::GestureSettings`] for the thresholds).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::input"]
pub enum Gesture {
    /// A lone finger resting on the screen. It lets go when the finger lifts, and is cut short
    /// (without the triggers a release fires) when the finger starts dragging or a second
    /// finger lands, so taps, double taps and long presses come from the usual triggers.
    Press,
    /// A lone finger moving: camera drag.
    Drag,
    /// Two fingers spreading or closing: zoom.
    Pinch,
    /// A quick one-finger stroke, by its dominant direction: dodge.
    Swipe(SwipeDirection),
}

/// The direction of a swipe, on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::input"]
pub enum SwipeDirection {
    Up,
    Down,
    Left,
    Right,
}

impl SwipeDirection {
    pub(crate) const COUNT: usize = 4;

    pub(crate) fn index(self) -> usize {
        self as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_to_scancodes_both_ways() {
        assert_eq!(Key::W.scancode(), Scancode::W);
        assert_eq!(Key::from_scancode(Scancode::SPACE), Key::Space);
        assert_eq!(Key::from_scancode(Scancode::RIGHT_GUI), Key::RightGui);
        // Keys without a name keep their usage ID.
        assert_eq!(Key::from_scancode(Scancode(100)), Key::Hid(100));
        assert_eq!(Key::Hid(100).scancode(), Scancode(100));
    }

    #[test]
    fn pad_buttons_cover_the_platform() {
        assert_eq!(PadButton::from_platform(GamepadButton::South), Some(PadButton::South));
        assert_eq!(PadButton::from_platform(GamepadButton::Misc6), Some(PadButton::Misc6));
        assert_eq!(PadButton::South.bit(), 0);
        assert!(PadButton::Misc6.bit() < 32, "button state is a u32 bitset");
    }

    #[test]
    fn mouse_buttons_beyond_the_bitset_are_untracked() {
        assert_eq!(MouseButton::from_platform(sapphire_platform::MouseButton::X2), MouseButton::Forward);
        assert_eq!(MouseButton::Other(6).bit(), Some(6));
        assert_eq!(MouseButton::Other(40).bit(), None);
    }
}
