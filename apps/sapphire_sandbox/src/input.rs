//! Maps raw platform input onto the orbit camera and the simulation.
//!
//! M0 stand-in for `sapphire_input` (M1, spec §5.18), which adds contexts, action mapping and
//! rebinding. The split it already follows:
//!
//! - **Camera input is presentation.** Pointer gestures apply immediately; held keys, sticks
//!   and triggers apply per frame for the real `dt`.
//! - **Simulation input is queued.** Presses become [`SimActions`] that the next fixed step
//!   consumes, so an input lands on a tick rather than whenever the OS delivered it (spec
//!   §4.2 step 1, rule R3), and replays stay deterministic.

use sapphire_platform::{
    FingerId, GamepadAxis, GamepadButton, MouseButton, Scancode, TouchEvent, TouchPhase,
};

use crate::orbit::OrbitCamera;

/// Radians per window point of mouse drag.
const MOUSE_ORBIT: f32 = 0.008;
/// Radians per window point of finger drag.
const TOUCH_ORBIT: f32 = 0.006;
/// Distance factor per wheel notch.
const WHEEL_ZOOM: f32 = 0.9;
/// Radians per second while an arrow key is held.
const KEY_ORBIT: f32 = 1.5;
/// Radians per second at full stick.
const STICK_ORBIT: f32 = 2.5;
/// Distance factor per second at full trigger.
const TRIGGER_ZOOM: f32 = 3.0;
const STICK_DEADZONE: f32 = 0.15;

/// Simulation actions requested since the last fixed step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SimActions {
    /// Flip world slow motion. Two presses before a step cancel out.
    pub toggle_slow_motion: bool,
    /// Freeze the turntable for a few ticks.
    pub hitstop: bool,
}

#[derive(Default)]
pub(crate) struct InputState {
    dragging: bool,
    /// Up to two tracked fingers with their last positions in window points.
    fingers: [Option<(FingerId, [f32; 2])>; 2],
    /// Held arrow keys: left, right, up, down.
    arrows: [bool; 4],
    stick: [f32; 2],
    triggers: [f32; 2],
    actions: SimActions,
}

impl InputState {
    pub fn key(&mut self, scancode: Scancode, down: bool, repeat: bool) {
        let press = down && !repeat;
        match scancode {
            Scancode::LEFT => self.arrows[0] = down,
            Scancode::RIGHT => self.arrows[1] = down,
            Scancode::UP => self.arrows[2] = down,
            Scancode::DOWN => self.arrows[3] = down,
            Scancode::SPACE if press => self.actions.toggle_slow_motion ^= true,
            Scancode::H if press => self.actions.hitstop = true,
            _ => {}
        }
    }

    pub fn gamepad_button(&mut self, button: GamepadButton, down: bool) {
        match button {
            GamepadButton::South if down => self.actions.toggle_slow_motion ^= true,
            GamepadButton::East if down => self.actions.hitstop = true,
            _ => {}
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

    /// One finger orbits; two fingers pinch-zoom. `window_size` is in window points: touch
    /// coordinates are normalized per axis, so gestures are measured after scaling back, or a
    /// wide screen would pinch-zoom on a rotation and pitch faster than it yaws.
    pub fn touch(&mut self, camera: &mut OrbitCamera, event: &TouchEvent, window_size: [f32; 2]) {
        let position = [event.x * window_size[0], event.y * window_size[1]];
        match event.phase {
            TouchPhase::Down => {
                // A finger we already track (its release was lost) restarts in place.
                let slot =
                    self.finger_index(event.finger).or_else(|| self.fingers.iter().position(Option::is_none));
                if let Some(slot) = slot {
                    self.fingers[slot] = Some((event.finger, position));
                }
            }
            TouchPhase::Moved => {
                let Some(index) = self.finger_index(event.finger) else { return };
                let previous = self.fingers[index].map_or(position, |(_, p)| p);
                match self.fingers[1 - index] {
                    Some((_, other)) => {
                        let (before, after) = (distance(other, previous), distance(other, position));
                        if before > f32::EPSILON && after > f32::EPSILON {
                            camera.zoom(before / after);
                        }
                    }
                    None => camera.rotate(
                        -(position[0] - previous[0]) * TOUCH_ORBIT,
                        (position[1] - previous[1]) * TOUCH_ORBIT,
                    ),
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

    /// Applies held keys, stick and triggers for `dt` real seconds.
    pub fn update(&self, camera: &mut OrbitCamera, dt: f32) {
        let held =
            |negative: bool, positive: bool| f32::from(u8::from(positive)) - f32::from(u8::from(negative));
        let [left, right, up, down] = self.arrows;
        let [stick_x, stick_y] = self.stick.map(deadzone);
        let yaw = held(right, left) * KEY_ORBIT - stick_x * STICK_ORBIT;
        let pitch = held(down, up) * KEY_ORBIT - stick_y * STICK_ORBIT;
        if yaw != 0.0 || pitch != 0.0 {
            camera.rotate(yaw * dt, pitch * dt);
        }
        let zoom = self.triggers[0] - self.triggers[1];
        if zoom != 0.0 {
            camera.zoom(TRIGGER_ZOOM.powf(zoom * dt));
        }
    }

    /// Takes the actions queued for the next fixed step.
    pub fn take_actions(&mut self) -> SimActions {
        std::mem::take(&mut self.actions)
    }

    /// Forgets held keys, buttons and fingers. Call when focus or the foreground is lost:
    /// their releases may never arrive.
    pub fn release_all(&mut self) {
        let actions = self.actions;
        *self = Self { actions, ..Self::default() };
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

    #[test]
    fn presses_queue_actions_for_the_next_step() {
        let mut input = InputState::default();
        input.key(Scancode::SPACE, true, false);
        input.key(Scancode::SPACE, true, true); // auto-repeat is ignored
        input.gamepad_button(GamepadButton::East, true);
        assert_eq!(input.take_actions(), SimActions { toggle_slow_motion: true, hitstop: true });
        assert_eq!(input.take_actions(), SimActions::default());

        // Two toggles before a step cancel out.
        input.key(Scancode::SPACE, true, false);
        input.gamepad_button(GamepadButton::South, true);
        assert!(!input.take_actions().toggle_slow_motion);
    }

    #[test]
    fn release_all_drops_held_state_but_keeps_queued_actions() {
        let mut input = InputState::default();
        input.key(Scancode::LEFT, true, false);
        input.key(Scancode::H, true, false);
        input.release_all();
        assert_eq!(input.arrows, [false; 4]);
        assert!(input.take_actions().hitstop);
    }
}
