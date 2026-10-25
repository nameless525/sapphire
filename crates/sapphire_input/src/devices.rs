//! Device state: what every key, button, axis and accumulator reads right now, and which kind
//! of device the player used last.

use sapphire_math::Vec2;
use sapphire_platform::{GamepadType, Scancode};

use crate::map::{Channel, SourceId};
use crate::source::{Gesture, MouseButton, PadAxis, PadButton};

/// Gamepads tracked at once; more are ignored until one disconnects.
pub const MAX_GAMEPADS: usize = 8;
/// Fingers tracked at once; more are ignored until one lifts.
pub const MAX_FINGERS: usize = 10;

/// Scancodes tracked: SDL's range.
const KEY_COUNT: usize = 512;

/// The kind of device the player used last, for prompt glyphs (spec §5.18, §6.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeviceKind {
    KeyboardMouse,
    Gamepad(GamepadFamily),
    Touch,
}

/// Which glyph set a gamepad's prompts use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GamepadFamily {
    /// A B X Y, as on Xbox controllers and the Steam Deck.
    Xbox,
    /// Cross, circle, square, triangle.
    PlayStation,
    /// A B X Y with A and B (and X and Y) swapped from the Xbox positions.
    Nintendo,
    /// An unrecognized controller: neutral, position-based glyphs.
    Generic,
}

impl GamepadFamily {
    pub fn of(kind: GamepadType) -> Self {
        match kind {
            GamepadType::Xbox360 | GamepadType::XboxOne | GamepadType::Steam => Self::Xbox,
            GamepadType::PlayStation3 | GamepadType::PlayStation4 | GamepadType::PlayStation5 => {
                Self::PlayStation
            }
            GamepadType::SwitchPro
            | GamepadType::JoyConLeft
            | GamepadType::JoyConRight
            | GamepadType::JoyConPair
            | GamepadType::GameCube => Self::Nintendo,
            _ => Self::Generic,
        }
    }
}

/// Stick or trigger travel that counts as the player using a gamepad, past any drift.
const PAD_ACTIVITY: f32 = 0.5;

#[derive(Clone, Copy, Debug, Default)]
struct Pad {
    connected: bool,
    family: Option<GamepadFamily>,
    buttons: u64,
    /// SDL's conventions: sticks −1..=1 with +y down, triggers 0..=1.
    axes: [f32; 6],
}

/// Motion summed between updates. `frame` is the update it belongs to: the sum restarts with
/// the first motion of a new update rather than at the update's start, so a source that keeps
/// moving reads continuously across the update boundary.
#[derive(Clone, Copy, Debug, Default)]
struct Motion {
    sum: Vec2,
    frame: u64,
    live: bool,
}

#[derive(Debug)]
pub(crate) struct Devices {
    keys: [u64; KEY_COUNT / 64],
    mouse: u64,
    pads: [Pad; MAX_GAMEPADS],
    motion: [Motion; Channel::COUNT],
    press: bool,
    swipes: [bool; crate::source::SwipeDirection::COUNT],
    virtuals: Vec<Vec2>,
    last: Option<DeviceKind>,
}

impl Devices {
    pub fn new(virtuals: usize) -> Self {
        Self {
            keys: [0; KEY_COUNT / 64],
            mouse: 0,
            pads: [Pad::default(); MAX_GAMEPADS],
            motion: [Motion::default(); Channel::COUNT],
            press: false,
            swipes: [false; crate::source::SwipeDirection::COUNT],
            virtuals: vec![Vec2::ZERO; virtuals],
            last: None,
        }
    }

    pub fn last(&self) -> Option<DeviceKind> {
        self.last
    }

    pub fn used(&mut self, kind: DeviceKind) {
        self.last = Some(kind);
    }

    pub fn gamepads(&self) -> usize {
        self.pads.iter().filter(|pad| pad.connected).count()
    }

    /// Sets a key; returns whether it changed.
    pub fn set_key(&mut self, scancode: Scancode, down: bool) -> bool {
        let index = usize::from(scancode.0);
        if index >= KEY_COUNT {
            return false;
        }
        set_bit(&mut self.keys[index / 64], (index % 64) as u32, down)
    }

    pub fn set_mouse(&mut self, button: MouseButton, down: bool) -> bool {
        button.bit().is_some_and(|bit| set_bit(&mut self.mouse, bit, down))
    }

    pub fn connect_pad(&mut self, pad: usize, kind: GamepadType) {
        if let Some(slot) = self.pads.get_mut(pad) {
            *slot = Pad { connected: true, family: Some(GamepadFamily::of(kind)), ..Pad::default() };
        }
    }

    /// Forgets a pad and everything it held; returns whether it held anything.
    pub fn disconnect_pad(&mut self, pad: usize) -> bool {
        let Some(slot) = self.pads.get_mut(pad) else { return false };
        let held = slot.buttons != 0 || slot.axes.iter().any(|&a| a != 0.0);
        *slot = Pad::default();
        held
    }

    pub fn set_pad_button(&mut self, pad: usize, button: PadButton, down: bool) -> bool {
        let Some(slot) = self.pads.get_mut(pad).filter(|slot| slot.connected) else { return false };
        let changed = set_bit(&mut slot.buttons, button.bit(), down);
        if down {
            self.last = Some(DeviceKind::Gamepad(slot.family.unwrap_or(GamepadFamily::Generic)));
        }
        changed
    }

    pub fn set_pad_axis(&mut self, pad: usize, axis: PadAxis, value: f32) -> bool {
        let Some(slot) = self.pads.get_mut(pad).filter(|slot| slot.connected) else { return false };
        let value = if value.is_finite() { value.clamp(-1.0, 1.0) } else { 0.0 };
        let old = std::mem::replace(&mut slot.axes[axis.index()], value);
        if value.abs() >= PAD_ACTIVITY {
            self.last = Some(DeviceKind::Gamepad(slot.family.unwrap_or(GamepadFamily::Generic)));
        }
        old != value
    }

    pub fn set_press(&mut self, down: bool) -> bool {
        std::mem::replace(&mut self.press, down) != down
    }

    pub fn set_swipe(&mut self, direction: crate::source::SwipeDirection, on: bool) {
        self.swipes[direction.index()] = on;
    }

    /// Starts over with a recompiled map's on-screen controls, which may be numbered
    /// differently.
    pub fn reset_virtuals(&mut self, count: usize) {
        self.virtuals = vec![Vec2::ZERO; count];
    }

    /// Sets an on-screen control; returns whether it changed.
    pub fn set_virtual(&mut self, index: usize, value: Vec2) -> bool {
        self.virtuals.get_mut(index).is_some_and(|slot| std::mem::replace(slot, value) != value)
    }

    /// Adds motion for `frame`; returns whether it is the first this frame.
    pub fn add_motion(&mut self, channel: Channel, delta: Vec2, frame: u64) -> bool {
        let motion = &mut self.motion[channel.index()];
        if motion.live && motion.frame == frame {
            motion.sum += delta;
            false
        } else {
            *motion = Motion { sum: delta, frame, live: true };
            true
        }
    }

    /// Lets go of every key, button, axis, press and on-screen control (motion is momentary
    /// and needs no release).
    pub fn release_all(&mut self) {
        self.keys = [0; KEY_COUNT / 64];
        self.mouse = 0;
        for pad in &mut self.pads {
            pad.buttons = 0;
            pad.axes = [0.0; 6];
        }
        self.press = false;
        self.swipes = [false; crate::source::SwipeDirection::COUNT];
        self.virtuals.fill(Vec2::ZERO);
    }

    /// What `source` reads, before modifiers, in the update numbered `frame`.
    pub fn value(&self, source: SourceId, frame: u64) -> Vec2 {
        let button = |on: bool| if on { Vec2::X } else { Vec2::ZERO };
        let pads = || self.pads.iter().filter(|pad| pad.connected);
        let axis = |pad: &Pad, axis: PadAxis| {
            let value = pad.axes[axis.index()];
            if axis.is_vertical() { -value } else { value }
        };
        let motion = |channel: Channel| {
            let motion = &self.motion[channel.index()];
            if motion.live && motion.frame == frame { motion.sum } else { Vec2::ZERO }
        };
        match source {
            SourceId::Key(code) => {
                let index = usize::from(code);
                button(index < KEY_COUNT && self.keys[index / 64] & (1 << (index % 64)) != 0)
            }
            SourceId::Mouse(bit) => button(bit.is_some_and(|bit| self.mouse & (1 << bit) != 0)),
            SourceId::Pad(bit) => button(pads().any(|pad| pad.buttons & (1 << bit) != 0)),
            SourceId::PadAxis(a) => {
                let value = pads()
                    .map(|pad| axis(pad, a))
                    .fold(0.0, |best: f32, v| if v.abs() > best.abs() { v } else { best });
                Vec2::new(value, 0.0)
            }
            SourceId::PadStick(stick) => {
                let (x, y) = stick.axes();
                pads().map(|pad| Vec2::new(axis(pad, x), axis(pad, y))).fold(Vec2::ZERO, |best, v| {
                    if v.length_squared() > best.length_squared() { v } else { best }
                })
            }
            SourceId::MouseMotion => {
                let delta = motion(Channel::Pointer);
                Vec2::new(delta.x, -delta.y)
            }
            SourceId::MouseWheel => Vec2::new(motion(Channel::Wheel).y, 0.0),
            SourceId::MouseWheelHorizontal => Vec2::new(motion(Channel::Wheel).x, 0.0),
            SourceId::Touch(Gesture::Press) => button(self.press),
            SourceId::Touch(Gesture::Drag) => motion(Channel::Drag),
            SourceId::Touch(Gesture::Pinch) => motion(Channel::Pinch),
            SourceId::Touch(Gesture::Swipe(direction)) => button(self.swipes[direction.index()]),
            SourceId::Virtual(control) => self.virtuals.get(control.index()).copied().unwrap_or(Vec2::ZERO),
        }
    }
}

/// Sets or clears `bit`; returns whether it changed.
fn set_bit(bits: &mut u64, bit: u32, on: bool) -> bool {
    let mask = 1 << bit;
    let old = *bits;
    *bits = if on { old | mask } else { old & !mask };
    *bits != old
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn several_pads_act_as_one() {
        let mut devices = Devices::new(0);
        devices.connect_pad(0, GamepadType::XboxOne);
        devices.connect_pad(1, GamepadType::PlayStation5);
        devices.set_pad_axis(0, PadAxis::LeftX, 0.3);
        devices.set_pad_axis(1, PadAxis::LeftX, -0.6);
        // SDL's down is the source's up.
        devices.set_pad_axis(1, PadAxis::LeftY, -0.8);
        assert_eq!(devices.value(SourceId::PadAxis(PadAxis::LeftX), 0), Vec2::new(-0.6, 0.0));
        assert_eq!(devices.value(SourceId::PadStick(crate::source::Stick::Left), 0), Vec2::new(-0.6, 0.8));
        assert_eq!(devices.last(), Some(DeviceKind::Gamepad(GamepadFamily::PlayStation)));

        devices.set_pad_button(0, PadButton::South, true);
        assert_eq!(devices.value(SourceId::Pad(PadButton::South.bit()), 0), Vec2::X);
        assert_eq!(devices.last(), Some(DeviceKind::Gamepad(GamepadFamily::Xbox)));
        assert!(devices.disconnect_pad(0));
        assert_eq!(devices.value(SourceId::Pad(PadButton::South.bit()), 0), Vec2::ZERO);
        assert!(!devices.set_pad_button(0, PadButton::South, true), "a disconnected pad is ignored");
    }

    #[test]
    fn motion_restarts_with_each_update() {
        let mut devices = Devices::new(0);
        assert!(devices.add_motion(Channel::Pointer, Vec2::new(1.0, 2.0), 0));
        assert!(!devices.add_motion(Channel::Pointer, Vec2::new(1.0, 2.0), 0));
        assert_eq!(devices.value(SourceId::MouseMotion, 0), Vec2::new(2.0, -4.0), "+y up");
        assert_eq!(devices.value(SourceId::MouseMotion, 1), Vec2::ZERO, "no motion in the next update");
        assert!(devices.add_motion(Channel::Pointer, Vec2::new(5.0, 0.0), 1));
        assert_eq!(devices.value(SourceId::MouseMotion, 1), Vec2::new(5.0, 0.0));
    }

    #[test]
    fn families_follow_the_controller() {
        assert_eq!(GamepadFamily::of(GamepadType::SwitchPro), GamepadFamily::Nintendo);
        assert_eq!(GamepadFamily::of(GamepadType::Standard), GamepadFamily::Generic);
        assert_eq!(GamepadFamily::of(GamepadType::Unknown), GamepadFamily::Generic);
    }
}
