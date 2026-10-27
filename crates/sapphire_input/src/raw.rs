//! [`RawInput`]: device input as the pipeline consumes it, and the adapter that produces it
//! from `sapphire_platform` events.

use sapphire_math::Vec2;
use sapphire_platform::{
    Event, FingerId, GamepadEvent, GamepadId, GamepadType, LifecycleEvent, Scancode, Timestamp, TouchPhase,
    WindowEvent,
};

use crate::devices::{MAX_FINGERS, MAX_GAMEPADS};
use crate::map::VirtualControl;
use crate::source::{MouseButton, PadAxis, PadButton};

/// One device input, in the device's own conventions.
///
/// [`InputSystem::handle_event`](crate::InputSystem::handle_event) translates platform events
/// into these; tests and replays push them directly with
/// [`InputSystem::push`](crate::InputSystem::push).
///
/// Gamepads and fingers are numbered by slot (below [`MAX_GAMEPADS`] and [`MAX_FINGERS`]),
/// which the adapter assigns as they connect or land.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RawInput {
    /// A key went down or up. Auto-repeats are not input.
    Key {
        scancode: Scancode,
        down: bool,
    },
    MouseButton {
        button: MouseButton,
        down: bool,
    },
    /// Pointer movement in window points, +y down.
    MouseMotion {
        delta: Vec2,
    },
    /// Scrolling in notches: +y away from the user, +x right.
    MouseWheel {
        delta: Vec2,
    },
    GamepadAdded {
        pad: usize,
        kind: GamepadType,
    },
    GamepadRemoved {
        pad: usize,
    },
    PadButton {
        pad: usize,
        button: PadButton,
        down: bool,
    },
    /// Sticks −1..=1 with +y down, triggers 0..=1, as SDL reports them.
    PadAxis {
        pad: usize,
        axis: PadAxis,
        value: f32,
    },
    /// A finger, at `position` in window points with +y down.
    Touch {
        finger: usize,
        phase: TouchPhase,
        position: Vec2,
    },
    /// An on-screen control's new value.
    Virtual {
        control: VirtualControl,
        value: Vec2,
    },
    /// Every held key, button, axis, finger and on-screen control lets go, firing nothing a
    /// release would: the window lost focus or the app left the foreground, so the real
    /// releases may never arrive. Sequences in progress are canceled too: a double tap waiting
    /// for its second press goes idle, so the first press after the return starts a new one.
    ReleaseAll,
}

/// Translates platform events, assigning gamepad and finger slots and scaling touches from
/// normalized coordinates to window points.
#[derive(Debug, Default)]
pub(crate) struct PlatformAdapter {
    /// The window's size in window points, which touch positions are scaled to.
    pub window_size: Vec2,
    pads: [Option<GamepadId>; MAX_GAMEPADS],
    fingers: [Option<FingerId>; MAX_FINGERS],
}

impl PlatformAdapter {
    pub fn translate(&mut self, event: &Event) -> Option<(Timestamp, RawInput)> {
        Some(match *event {
            Event::Key(key) if !key.repeat => {
                (key.timestamp, RawInput::Key { scancode: key.scancode, down: key.down })
            }
            Event::MouseButton(button) => (
                button.timestamp,
                RawInput::MouseButton {
                    button: MouseButton::from_platform(button.button),
                    down: button.down,
                },
            ),
            Event::MouseMotion(motion) => {
                (motion.timestamp, RawInput::MouseMotion { delta: Vec2::new(motion.dx, motion.dy) })
            }
            Event::MouseWheel(wheel) => {
                (wheel.timestamp, RawInput::MouseWheel { delta: Vec2::new(wheel.x, wheel.y) })
            }
            Event::Touch(touch) => {
                let finger = slot(&mut self.fingers, touch.finger, touch.phase == TouchPhase::Down)?;
                if matches!(touch.phase, TouchPhase::Up | TouchPhase::Canceled) {
                    self.fingers[finger] = None;
                }
                let position = touch_position(touch.x, touch.y, self.window_size);
                (touch.timestamp, RawInput::Touch { finger, phase: touch.phase, position })
            }
            Event::Gamepad { timestamp, gamepad, event } => {
                let added = matches!(event, GamepadEvent::Added { .. });
                let pad = slot(&mut self.pads, gamepad, added)?;
                let input = match event {
                    GamepadEvent::Added { kind } => RawInput::GamepadAdded { pad, kind },
                    GamepadEvent::Removed => {
                        self.pads[pad] = None;
                        RawInput::GamepadRemoved { pad }
                    }
                    GamepadEvent::Button { button, down } => {
                        RawInput::PadButton { pad, button: PadButton::from_platform(button)?, down }
                    }
                    GamepadEvent::Axis { axis, value } => {
                        RawInput::PadAxis { pad, axis: PadAxis::from_platform(axis), value }
                    }
                };
                (timestamp, input)
            }
            Event::Window { event: WindowEvent::Resized { width, height }, .. } => {
                self.window_size = Vec2::new(width as f32, height as f32);
                return None;
            }
            Event::Window { timestamp, event: WindowEvent::FocusLost, .. }
            | Event::Lifecycle { timestamp, event: LifecycleEvent::WillEnterBackground } => {
                // Here, not when the release is applied: fingers that land after it, in the
                // same frame, take slots before then.
                self.release_fingers();
                (timestamp, RawInput::ReleaseAll)
            }
            _ => return None,
        })
    }

    /// Forgets every finger. Once focus or the foreground is lost their releases may never
    /// come, and fingers landing later need the slots. A late event from an old finger finds
    /// no slot and is dropped, like the gestures it belonged to. Gamepads keep their slots:
    /// they are still connected.
    pub fn release_fingers(&mut self) {
        self.fingers = [None; MAX_FINGERS];
    }
}

/// The slot `id` occupies, claiming a free one if `claim` (a finger whose release was lost
/// keeps its slot until focus or the foreground is lost). `None` when it has none, or all are
/// taken.
fn slot<T: Copy + PartialEq>(slots: &mut [Option<T>], id: T, claim: bool) -> Option<usize> {
    if let Some(index) = slots.iter().position(|&s| s == Some(id)) {
        return Some(index);
    }
    let free = slots.iter().position(Option::is_none).filter(|_| claim)?;
    slots[free] = Some(id);
    Some(free)
}

/// Touch coordinates are normalized per axis; gestures are measured in window points, or a
/// wide screen would pinch on a rotation and drag faster one way than the other.
fn touch_position(x: f32, y: f32, window_size: Vec2) -> Vec2 {
    Vec2::new(x, y) * window_size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_are_claimed_reused_and_bounded() {
        let mut slots = [None; 2];
        assert_eq!(slot(&mut slots, 7, true), Some(0));
        assert_eq!(slot(&mut slots, 7, true), Some(0), "the same id keeps its slot");
        assert_eq!(slot(&mut slots, 8, false), None, "only a connect or a landing claims a slot");
        assert_eq!(slot(&mut slots, 8, true), Some(1));
        assert_eq!(slot(&mut slots, 9, true), None, "all taken");
    }

    #[test]
    fn touches_scale_to_window_points() {
        assert_eq!(touch_position(0.5, 0.25, Vec2::new(1920.0, 1080.0)), Vec2::new(960.0, 270.0));
    }

    /// The slot a touch by finger `id` lands in, if it gets one.
    fn slot_of(adapter: &mut PlatformAdapter, id: u64, phase: TouchPhase) -> Option<usize> {
        let touch = sapphire_platform::TouchEvent {
            timestamp: Timestamp::from_nanos(1),
            window: sapphire_platform::WindowId::from_raw(1),
            finger: FingerId::new(sapphire_platform::TouchDeviceId::from_raw(1), id),
            phase,
            x: 0.5,
            y: 0.5,
            pressure: 1.0,
        };
        match adapter.translate(&Event::Touch(touch)) {
            Some((_, RawInput::Touch { finger, .. })) => Some(finger),
            _ => None,
        }
    }

    #[test]
    fn losing_focus_frees_every_finger_slot() {
        // Review FI-001: the releases of fingers down when focus goes may never come, so their
        // slots are freed as the loss is translated, before any later finger lands.
        let mut adapter = PlatformAdapter::default();
        assert_eq!(slot_of(&mut adapter, 7, TouchPhase::Down), Some(0));
        assert_eq!(slot_of(&mut adapter, 8, TouchPhase::Down), Some(1));
        let at = Timestamp::from_nanos(5);
        let lost = Event::Window {
            timestamp: at,
            window: sapphire_platform::WindowId::from_raw(1),
            event: WindowEvent::FocusLost,
        };
        assert_eq!(adapter.translate(&lost), Some((at, RawInput::ReleaseAll)));
        assert_eq!(slot_of(&mut adapter, 9, TouchPhase::Down), Some(0), "the lost fingers' slots are free");
        assert_eq!(slot_of(&mut adapter, 7, TouchPhase::Up), None, "a late release finds no finger");
        assert_eq!(slot_of(&mut adapter, 8, TouchPhase::Moved), None);
        assert_eq!(slot_of(&mut adapter, 9, TouchPhase::Up), Some(0));
        for id in 0..MAX_FINGERS {
            assert_eq!(
                slot_of(&mut adapter, 100 + id as u64, TouchPhase::Down),
                Some(id),
                "every slot is free"
            );
        }

        // Going to the background frees them too.
        let background = Event::Lifecycle { timestamp: at, event: LifecycleEvent::WillEnterBackground };
        assert_eq!(adapter.translate(&background), Some((at, RawInput::ReleaseAll)));
        assert_eq!(slot_of(&mut adapter, 200, TouchPhase::Down), Some(0));
    }

    #[test]
    fn focus_and_background_release_everything() {
        let mut adapter = PlatformAdapter::default();
        let at = Timestamp::from_nanos(5);
        let event = Event::Lifecycle { timestamp: at, event: LifecycleEvent::WillEnterBackground };
        assert_eq!(adapter.translate(&event), Some((at, RawInput::ReleaseAll)));
        let event = Event::Lifecycle { timestamp: at, event: LifecycleEvent::LowMemory };
        assert_eq!(adapter.translate(&event), None);
        assert_eq!(adapter.translate(&Event::Quit { timestamp: at }), None);
    }
}
