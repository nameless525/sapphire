//! A desktop window's events, as egui hears them: the small part of `sapphire_debug`'s adapter a tool with
//! a mouse and a keyboard needs. That one also routes touches between an overlay and a game, and lives in a
//! crate with a renderer and a world behind it, which the editor doesn't link. Plain data in and out, so
//! it's tested with no window.

use egui::{Key, PointerButton, Pos2, Vec2};
use sapphire_platform::{
    Event, KeyEvent, Keycode, LifecycleEvent, Modifiers, MouseButton, Os, Scancode, WindowEvent,
};

/// Events waiting for the next pass.
#[derive(Debug)]
pub struct WindowInput {
    events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
    /// Points per window coordinate.
    points_per_unit: f32,
    /// Whether Cmd is egui's `command`.
    command_is_gui: bool,
    /// Whether anything has come since the last pass.
    woken: bool,
    /// Whether the window lost focus since the last pass.
    released: bool,
}

impl Default for WindowInput {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            modifiers: egui::Modifiers::NONE,
            points_per_unit: 1.0,
            command_is_gui: matches!(Os::CURRENT, Os::MacOs | Os::Ios),
            woken: true,
            released: false,
        }
    }
}

impl WindowInput {
    /// Takes the window's scale, for the events that follow.
    pub fn set_points_per_unit(&mut self, points_per_unit: f32) {
        self.points_per_unit = points_per_unit;
    }

    fn at(&self, x: f32, y: f32) -> Pos2 {
        Pos2::new(x * self.points_per_unit, y * self.points_per_unit)
    }

    /// Whether a pass has something new to read.
    #[must_use]
    pub fn woken(&self) -> bool {
        self.woken
    }

    /// Takes what's queued for a pass.
    pub fn take(&mut self) -> Vec<egui::Event> {
        self.woken = false;
        std::mem::take(&mut self.events)
    }

    /// Whether the window lost focus since the last time this was asked, so nothing is held any more.
    pub fn take_released(&mut self) -> bool {
        std::mem::take(&mut self.released)
    }

    /// Translates a platform event.
    pub fn push(&mut self, event: &Event) {
        self.woken = true;
        match *event {
            Event::Key(key) => self.key(&key),
            Event::Text(text) => {
                let text: String = text.text().chars().filter(|c| !c.is_control()).collect();
                if !text.is_empty() {
                    self.events.push(egui::Event::Text(text));
                }
            }
            Event::MouseMotion(motion) => {
                self.events.push(egui::Event::PointerMoved(self.at(motion.x, motion.y)));
            }
            Event::MouseButton(button) => {
                let Some(pointer) = pointer_button(button.button) else { return };
                self.events.push(egui::Event::PointerButton {
                    pos: self.at(button.x, button.y),
                    button: pointer,
                    pressed: button.down,
                    modifiers: self.modifiers,
                });
            }
            Event::MouseWheel(wheel) => self.events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: Vec2::new(wheel.x, wheel.y),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
                modifiers: self.modifiers,
            }),
            // Nothing stays held across a lost focus: a drag ends where it is, and is undoable.
            Event::Window { event: WindowEvent::FocusLost, .. }
            | Event::Lifecycle { event: LifecycleEvent::WillEnterBackground, .. } => {
                self.modifiers = egui::Modifiers::NONE;
                self.released = true;
                self.events.push(egui::Event::PointerGone);
                self.events.push(egui::Event::WindowFocused(false));
            }
            Event::Window { event: WindowEvent::FocusGained, .. } => {
                self.events.push(egui::Event::WindowFocused(true));
            }
            _ => {}
        }
    }

    fn key(&mut self, key: &KeyEvent) {
        self.modifiers = modifiers(key.modifiers, self.command_is_gui);
        let Some(logical) = logical_key(key.keycode, key.scancode) else {
            self.events.push(egui::Event::ModifiersChanged(self.modifiers));
            return;
        };
        self.events.push(egui::Event::Key {
            key: logical,
            physical_key: None,
            pressed: key.down,
            repeat: key.repeat,
            modifiers: self.modifiers,
        });
    }
}

fn modifiers(held: Modifiers, command_is_gui: bool) -> egui::Modifiers {
    egui::Modifiers {
        alt: held.alt,
        ctrl: held.ctrl,
        shift: held.shift,
        mac_cmd: command_is_gui && held.gui,
        command: if command_is_gui { held.gui } else { held.ctrl },
    }
}

fn pointer_button(button: MouseButton) -> Option<PointerButton> {
    Some(match button {
        MouseButton::Left => PointerButton::Primary,
        MouseButton::Right => PointerButton::Secondary,
        MouseButton::Middle => PointerButton::Middle,
        MouseButton::X1 => PointerButton::Extra1,
        MouseButton::X2 => PointerButton::Extra2,
        MouseButton::Other(_) => return None,
    })
}

/// The key as the layout names it: the character it types, or the key that types none in that position.
fn logical_key(keycode: Keycode, scancode: Scancode) -> Option<Key> {
    let typed = keycode.to_char().and_then(|c| Key::from_name(&c.to_ascii_uppercase().to_string()));
    typed.or_else(|| {
        Some(match scancode {
            Scancode::RETURN => Key::Enter,
            Scancode::ESCAPE => Key::Escape,
            Scancode::BACKSPACE => Key::Backspace,
            Scancode::TAB => Key::Tab,
            Scancode::SPACE => Key::Space,
            Scancode::HOME => Key::Home,
            Scancode::END => Key::End,
            Scancode::PAGE_UP => Key::PageUp,
            Scancode::PAGE_DOWN => Key::PageDown,
            Scancode::DELETE => Key::Delete,
            Scancode::LEFT => Key::ArrowLeft,
            Scancode::RIGHT => Key::ArrowRight,
            Scancode::UP => Key::ArrowUp,
            Scancode::DOWN => Key::ArrowDown,
            _ => return None,
        })
    })
}

#[cfg(test)]
mod tests {
    use sapphire_platform::{MouseButtonEvent, MouseMotionEvent, Timestamp, WindowId};

    use super::*;

    fn key(scancode: Scancode, keycode: u32, held: Modifiers) -> Event {
        Event::Key(KeyEvent {
            timestamp: Timestamp::from_nanos(0),
            window: WindowId::from_raw(1),
            scancode,
            keycode: Keycode(keycode),
            modifiers: held,
            down: true,
            repeat: false,
        })
    }

    fn keys_of(events: &[egui::Event]) -> Vec<Key> {
        events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Key { key, .. } => Some(*key),
                _ => None,
            })
            .collect()
    }

    /// The keys the editor's shortcuts read, each by what SDL sends for it.
    #[test]
    fn the_shortcuts_keys_arrive() {
        let mut input = WindowInput::default();
        let none = Modifiers::default();
        input.push(&key(Scancode::S, u32::from('s'), Modifiers { gui: true, ..none }));
        input.push(&key(Scancode::Z, u32::from('z'), none));
        input.push(&key(Scancode::LEFT_BRACKET, u32::from('['), none));
        input.push(&key(Scancode::RIGHT_BRACKET, u32::from(']'), none));
        input.push(&key(Scancode::COMMA, u32::from(','), none));
        input.push(&key(Scancode::PERIOD, u32::from('.'), none));
        input.push(&key(Scancode::SPACE, u32::from(' '), none));
        input.push(&key(Scancode::DELETE, (1 << 30) | 76, none));
        input.push(&key(Scancode::BACKSPACE, 8, none));
        input.push(&key(Scancode::LEFT, (1 << 30) | 80, none));
        input.push(&key(Scancode::RIGHT, (1 << 30) | 79, none));
        input.push(&key(Scancode::HOME, (1 << 30) | 74, none));
        input.push(&key(Scancode::END, (1 << 30) | 77, none));
        let events = input.take();
        assert_eq!(
            keys_of(&events),
            [
                Key::S,
                Key::Z,
                Key::OpenBracket,
                Key::CloseBracket,
                Key::Comma,
                Key::Period,
                Key::Space,
                Key::Delete,
                Key::Backspace,
                Key::ArrowLeft,
                Key::ArrowRight,
                Key::Home,
                Key::End,
            ]
        );
    }

    #[test]
    fn command_is_the_platforms_own() {
        let held = Modifiers { gui: true, ..Modifiers::default() };
        assert!(modifiers(held, true).command && modifiers(held, true).mac_cmd);
        assert!(!modifiers(held, false).command);
        let held = Modifiers { ctrl: true, ..Modifiers::default() };
        assert!(modifiers(held, false).command && !modifiers(held, true).command);
    }

    #[test]
    fn the_mouse_arrives_in_points() {
        let mut input = WindowInput::default();
        input.set_points_per_unit(0.5);
        let (timestamp, window) = (Timestamp::from_nanos(0), WindowId::from_raw(1));
        input.push(&Event::MouseMotion(MouseMotionEvent {
            timestamp,
            window,
            x: 100.0,
            y: 40.0,
            dx: 0.0,
            dy: 0.0,
        }));
        input.push(&Event::MouseButton(MouseButtonEvent {
            timestamp,
            window,
            button: MouseButton::Left,
            down: true,
            clicks: 1,
            x: 100.0,
            y: 40.0,
        }));
        assert!(input.woken());
        let events = input.take();
        assert!(!input.woken());
        assert!(matches!(events[0], egui::Event::PointerMoved(at) if at == Pos2::new(50.0, 20.0)));
        assert!(matches!(
            events[1],
            egui::Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, .. }
                if pos == Pos2::new(50.0, 20.0)
        ));
    }

    #[test]
    fn a_lost_focus_lets_everything_go() {
        let mut input = WindowInput::default();
        let held = Modifiers { shift: true, ..Modifiers::default() };
        input.push(&key(Scancode::S, u32::from('s'), held));
        input.push(&Event::Window {
            timestamp: Timestamp::from_nanos(0),
            window: WindowId::from_raw(1),
            event: WindowEvent::FocusLost,
        });
        assert!(input.take_released());
        assert!(!input.take_released(), "once");
        assert_eq!(input.modifiers, egui::Modifiers::NONE);
        let events = input.take();
        assert!(events.iter().any(|event| matches!(event, egui::Event::PointerGone)));
        assert!(events.iter().any(|event| matches!(event, egui::Event::WindowFocused(false))));
    }
}
