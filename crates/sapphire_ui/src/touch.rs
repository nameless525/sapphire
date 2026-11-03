//! What the touch zones read, as plain values: what `CombatHud` draws, and what an app's bindings take. The
//! zones themselves are `game_ui`'s.

use std::time::Duration;

use sapphire_math::Vec2;
use sapphire_platform::Timestamp;

/// An on-screen button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Attack,
    Dodge,
    Skill,
}

impl Button {
    pub const ALL: [Self; 3] = [Self::Attack, Self::Dodge, Self::Skill];

    pub const fn index(self) -> usize {
        self as usize
    }
}

/// What happened to a button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ButtonEventKind {
    /// A finger landed on it.
    Pressed,
    /// It has been held for its hold time (`game_ui`'s attack hold).
    Held,
    /// Its finger lifted.
    Released,
    /// It let go with nothing a release would fire: the OS took the finger back, or focus or the
    /// foreground was lost.
    Canceled,
}

/// One edge on a button, in the order they happened.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ButtonEvent {
    pub button: Button,
    pub kind: ButtonEventKind,
    /// When, on the platform's event clock.
    pub at: Timestamp,
    /// How far its thumb had moved from where it landed, in points, +y up.
    pub drag: Vec2,
}

/// A button's state as of the last update.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ButtonState {
    /// A finger holds it.
    pub down: bool,
    /// When that finger landed; meaningful while `down`.
    pub pressed_at: Timestamp,
    /// How far the thumb has moved from where it landed, in points, +y up. Zero when not `down`.
    pub drag: Vec2,
    /// The thumb has gone past the drag slop since it landed, and stays so until it lifts.
    pub dragging: bool,
    /// The thumb is inside the hit circle. A thumb outside it still holds the button.
    pub over: bool,
    /// Its hold time has passed since it was pressed.
    pub held: bool,
    /// It was pressed since the last update, whether or not it's still down.
    pub pressed: bool,
    /// It was released since the last update.
    pub released: bool,
    /// It was canceled since the last update.
    pub canceled: bool,
}

impl ButtonState {
    /// How long it has been held at `now`, if it's down.
    pub fn held_for(&self, now: Timestamp) -> Option<Duration> {
        self.down.then(|| now.saturating_duration_since(self.pressed_at))
    }
}

/// The floating stick's state as of the last update.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StickState {
    /// A thumb is on it.
    pub active: bool,
    /// Where it points and how far, within the unit circle, +y up, past its dead zone.
    pub value: Vec2,
    /// Its middle, in points: under the thumb at first, following it if it runs past the radius.
    pub center: Vec2,
    /// The thumb, in points.
    pub thumb: Vec2,
}

/// What the app reads from the touch controls after each update: plain values, for its bindings or
/// its intents.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TouchActions {
    pub stick: StickState,
    /// How far the camera's finger moved since the last update, in points, +y up.
    pub camera_delta: Vec2,
    pub buttons: [ButtonState; Button::ALL.len()],
    /// Every button's edges since the last update, in the order they happened.
    pub events: Vec<ButtonEvent>,
}

impl TouchActions {
    pub fn button(&self, button: Button) -> &ButtonState {
        &self.buttons[button.index()]
    }
}
