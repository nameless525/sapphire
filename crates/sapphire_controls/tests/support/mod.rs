//! Synthetic input for the tests: no window and no device, as `sapphire_input` is meant to be driven.
#![allow(dead_code)] // Each test binary uses its own part of this.

use sapphire_controls::CombatControls;
use sapphire_gameplay::{Buttons, Controls};
use sapphire_input::{MouseButton, PadAxis, PadButton, RawInput, Timestamp, VirtualControl};
use sapphire_math::Vec2;
use sapphire_platform::{
    Event, GamepadType, KeyEvent, Keycode, LifecycleEvent, Modifiers, Scancode, WindowEvent, WindowId,
};

/// Android's Back, as SDL's `SDL_SCANCODE_AC_BACK`.
pub const BACK: Scancode = Scancode(282);

pub const L: Buttons = Buttons::LIGHT;
pub const H: Buttons = Buttons::HEAVY;
pub const D: Buttons = Buttons::DODGE;
pub const N: Buttons = Buttons::NONE;

pub fn ms(ms: u64) -> Timestamp {
    Timestamp::from_nanos(ms * 1_000_000)
}

pub fn key(scancode: Scancode, down: bool) -> RawInput {
    RawInput::Key { scancode, down }
}

pub fn pad(button: PadButton, down: bool) -> RawInput {
    RawInput::PadButton { pad: 0, button, down }
}

pub fn axis(axis: PadAxis, value: f32) -> RawInput {
    RawInput::PadAxis { pad: 0, axis, value }
}

pub fn mouse(button: MouseButton, down: bool) -> RawInput {
    RawInput::MouseButton { button, down }
}

pub fn control(control: VirtualControl, down: bool) -> RawInput {
    RawInput::Virtual { control, value: if down { Vec2::X } else { Vec2::ZERO } }
}

/// How long a frame takes in these tests, and so how many inputs fit in one, a millisecond apart.
pub const FRAME_MS: u64 = 16;

/// [`CombatControls`] with a pad connected, one frame in.
pub struct Rig {
    pub controls: CombatControls,
    /// The start of the next frame, in milliseconds.
    pub now: u64,
}

impl Rig {
    pub fn new() -> Self {
        Self::on(CombatControls::with_default_bindings().expect("the default bindings"))
    }

    pub fn on(mut controls: CombatControls) -> Self {
        controls.input_mut().push(ms(0), RawInput::GamepadAdded { pad: 0, kind: GamepadType::XboxOne });
        controls.update(ms(FRAME_MS));
        Self { controls, now: FRAME_MS }
    }

    /// One frame: these inputs, a millisecond apart, then its update.
    pub fn frame(&mut self, inputs: &[RawInput]) {
        assert!((inputs.len() as u64) < FRAME_MS, "they have to fit in the frame");
        for (offset, input) in inputs.iter().enumerate() {
            self.controls.input_mut().push(ms(self.now + 1 + offset as u64), *input);
        }
        self.finish_frame();
    }

    /// Ends a frame whose input has gone in some other way.
    pub fn finish_frame(&mut self) {
        self.now += FRAME_MS;
        self.controls.update(ms(self.now));
    }

    /// What the next `count` steps read.
    pub fn steps(&mut self, count: usize) -> Vec<Buttons> {
        (0..count).map(|_| self.controls.next_actions().buttons).collect()
    }

    /// A platform key event, within the current frame.
    pub fn platform_key(&mut self, offset: u64, scancode: Scancode, down: bool) {
        self.controls.handle_event(&Event::Key(KeyEvent {
            timestamp: ms(self.now + offset),
            window: WindowId::from_raw(1),
            scancode,
            keycode: Keycode(0),
            modifiers: Modifiers::default(),
            down,
            repeat: false,
        }));
    }

    /// Focus lost as SDL reports it, within the current frame: a key-up for every held key, then the
    /// loss, all at once (review WA-1).
    pub fn focus_lost(&mut self, offset: u64, held: &[Scancode]) {
        for &scancode in held {
            self.platform_key(offset, scancode, false);
        }
        self.controls.handle_event(&Event::Window {
            timestamp: ms(self.now + offset),
            window: WindowId::from_raw(1),
            event: WindowEvent::FocusLost,
        });
    }

    /// Going to the background, within the current frame.
    pub fn backgrounded(&mut self, offset: u64) {
        self.controls.handle_event(&Event::Lifecycle {
            timestamp: ms(self.now + offset),
            event: LifecycleEvent::WillEnterBackground,
        });
    }
}

/// Follows a run of steps as the simulation does, and counts what it takes for presses.
#[derive(Default)]
pub struct Edges {
    controls: Controls,
    /// Presses seen, by bit of [`Buttons`].
    pub by_bit: [u32; 16],
}

impl Edges {
    pub fn see(&mut self, buttons: Buttons) {
        self.controls.previous = self.controls.current;
        self.controls.current.buttons = buttons;
        let pressed = self.controls.pressed().bits();
        for (bit, count) in self.by_bit.iter_mut().enumerate() {
            if pressed & (1 << bit) != 0 {
                *count += 1;
            }
        }
    }

    pub fn total(&self) -> u32 {
        self.by_bit.iter().sum()
    }
}
