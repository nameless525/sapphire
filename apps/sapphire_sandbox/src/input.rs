//! Maps raw platform input onto the orbit camera.
//!
//! M0 stand-in for `sapphire_input` (M1, spec §5.18), which adds contexts, action mapping
//! and rebinding. Pointer gestures apply immediately; held axes apply per frame.

use sapphire_platform::{FingerId, GamepadAxis, MouseButton, TouchEvent, TouchPhase};

use crate::orbit::OrbitCamera;

/// Radians per window point of mouse drag.
const MOUSE_ORBIT: f32 = 0.008;
/// Radians per full-screen-width finger drag.
const TOUCH_ORBIT: f32 = 4.0;
/// Distance factor per wheel notch.
const WHEEL_ZOOM: f32 = 0.9;
/// Radians per keypress.
const KEY_ORBIT: f32 = 0.2;
/// Radians per second at full stick.
const STICK_ORBIT: f32 = 2.5;
/// Distance factor per second at full trigger.
const TRIGGER_ZOOM: f32 = 3.0;
const STICK_DEADZONE: f32 = 0.15;

#[derive(Default)]
pub(crate) struct InputState {
    dragging: bool,
    /// Up to two tracked fingers with their last normalized positions.
    fingers: [Option<(FingerId, [f32; 2])>; 2],
    stick: [f32; 2],
    triggers: [f32; 2],
}

impl InputState {
    pub fn mouse_button(&mut self, button: MouseButton, down: bool) {
        if button == MouseButton::Left {
            self.dragging = down;
        }
    }

    pub fn mouse_motion(&self, camera: &mut OrbitCamera, dx: f32, dy: f32) {
        if self.dragging {
            camera.rotate(-dx * MOUSE_ORBIT, dy * MOUSE_ORBIT);
        }
    }

    pub fn mouse_wheel(camera: &mut OrbitCamera, notches: f32) {
        camera.zoom(WHEEL_ZOOM.powf(notches));
    }

    /// Arrow-key orbit: `(dx, dy)` in steps.
    pub fn key_orbit(camera: &mut OrbitCamera, dx: f32, dy: f32) {
        camera.rotate(dx * KEY_ORBIT, dy * KEY_ORBIT);
    }

    /// One finger orbits; two fingers pinch-zoom.
    pub fn touch(&mut self, camera: &mut OrbitCamera, event: &TouchEvent) {
        let position = [event.x, event.y];
        match event.phase {
            TouchPhase::Down => {
                if let Some(slot) = self.fingers.iter_mut().find(|slot| slot.is_none()) {
                    *slot = Some((event.finger, position));
                }
            }
            TouchPhase::Moved => {
                let Some(index) = self.finger_index(event.finger) else { return };
                let [a, b] = self.fingers;
                match (a, b) {
                    (Some((_, a)), Some((_, b))) => {
                        let other = if index == 0 { b } else { a };
                        let before = distance(other, self.fingers[index].map_or(position, |(_, p)| p));
                        let after = distance(other, position);
                        if before > f32::EPSILON && after > f32::EPSILON {
                            camera.zoom(before / after);
                        }
                    }
                    _ => {
                        let previous = self.fingers[index].map_or(position, |(_, p)| p);
                        camera.rotate(
                            -(position[0] - previous[0]) * TOUCH_ORBIT,
                            (position[1] - previous[1]) * TOUCH_ORBIT,
                        );
                    }
                }
                self.fingers[index] = Some((event.finger, position));
            }
            TouchPhase::Up | TouchPhase::Canceled => {
                if let Some(index) = self.finger_index(event.finger) {
                    self.fingers[index] = None;
                }
            }
        }
    }

    pub fn gamepad_axis(&mut self, axis: GamepadAxis, value: f32) {
        match axis {
            GamepadAxis::RightX => self.stick[0] = value,
            GamepadAxis::RightY => self.stick[1] = value,
            GamepadAxis::LeftTrigger => self.triggers[0] = value,
            GamepadAxis::RightTrigger => self.triggers[1] = value,
            GamepadAxis::LeftX | GamepadAxis::LeftY => {}
        }
    }

    /// Applies held stick and trigger input for `dt` real seconds.
    pub fn update(&self, camera: &mut OrbitCamera, dt: f32) {
        let [x, y] = self.stick.map(deadzone);
        if x != 0.0 || y != 0.0 {
            camera.rotate(-x * STICK_ORBIT * dt, -y * STICK_ORBIT * dt);
        }
        let zoom = self.triggers[0] - self.triggers[1];
        if zoom != 0.0 {
            camera.zoom(TRIGGER_ZOOM.powf(zoom * dt));
        }
    }

    fn finger_index(&self, finger: FingerId) -> Option<usize> {
        self.fingers.iter().position(|slot| slot.is_some_and(|(id, _)| id == finger))
    }
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn deadzone(value: f32) -> f32 {
    if value.abs() < STICK_DEADZONE {
        0.0
    } else {
        (value - STICK_DEADZONE * value.signum()) / (1.0 - STICK_DEADZONE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadzone_rescales_to_full_range() {
        assert_eq!(deadzone(0.1), 0.0);
        assert_eq!(deadzone(-0.1), 0.0);
        assert!((deadzone(1.0) - 1.0).abs() < 1e-6);
        assert!((deadzone(-1.0) + 1.0).abs() < 1e-6);
    }
}
