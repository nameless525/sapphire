//! The SDL input adapter (spec §5.20): `sapphire_platform` events into egui's input, in points.
//!
//! - **Coordinates.** SDL reports mouse positions in window coordinates and touches normalized
//!   to the window; egui works in points, `pixels_per_point` pixels each (the window's display
//!   scale times the overlay's zoom). [`WindowMetrics`] carries what the conversion needs.
//! - **Keys.** egui's `key` is the logical key, so shortcuts follow the keyboard layout
//!   (`Cmd+A` on AZERTY is the key labelled A): character keys come from the keycode, the rest
//!   from the scancode, which is also every key's `physical_key`.
//! - **Modifiers.** egui's `command` is Cmd on Apple platforms and Ctrl elsewhere.
//! - **Touch.** Fingers the overlay owns become egui touches, and the first of them also drives
//!   egui's pointer, which is what widgets respond to.
//! - **Text** arrives as [`Event::Text`], only while the window takes text input; the overlay
//!   asks for it while a text field has focus ([`crate::OverlayOutput::text_input`]).
//!
//! Not translated yet: clipboard copy and paste (the platform has no clipboard API yet), IME
//! composition previews, and cursor shapes.

use egui::{Key, PointerButton, Pos2, Vec2};
use sapphire_platform::{
    Event, KeyEvent, Keycode, Modifiers, MouseButton, Os, Rect, Scancode, Size, TouchEvent, TouchPhase,
    Window, WindowEvent,
};

/// What converting window coordinates to egui points needs, taken fresh each frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowMetrics {
    /// The drawable size in pixels: the overlay's target.
    pub pixel_size: Size,
    /// The size in window coordinates, which mouse positions use.
    pub size: Size,
    /// Pixels per point the OS asks UI to use (2 on a Retina display, 1.5 at 150% on Windows).
    pub display_scale: f32,
    /// The part of the window clear of notches and system bars, in window coordinates.
    pub safe_area: Rect,
    /// Whether the window has keyboard focus.
    pub focused: bool,
}

impl WindowMetrics {
    /// The metrics of `window` now.
    pub fn of(window: &Window, focused: bool) -> Self {
        Self {
            pixel_size: window.pixel_size(),
            size: window.size(),
            display_scale: window.display_scale(),
            safe_area: window.safe_area(),
            focused,
        }
    }

    /// Metrics for an offscreen target of `width` × `height` pixels, drawn at `display_scale`
    /// pixels per point: window coordinates are pixels.
    pub fn offscreen(width: u32, height: u32, display_scale: f32) -> Self {
        let size = Size { width, height };
        Self {
            pixel_size: size,
            size,
            display_scale,
            safe_area: Rect { x: 0, y: 0, width, height },
            focused: true,
        }
    }

    /// Pixels per window coordinate.
    pub fn pixel_density(&self) -> f32 {
        if self.size.width == 0 { 1.0 } else { self.pixel_size.width as f32 / self.size.width as f32 }
    }
}

/// egui events built from platform events, waiting for the next frame.
#[derive(Debug)]
pub(crate) struct EguiInput {
    events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
    /// Points per window coordinate.
    points_per_unit: f32,
    /// The window's size in window coordinates, for normalized touch positions.
    window_size: Vec2,
    /// The finger driving egui's pointer, by the router's finger slot.
    pointer_finger: Option<usize>,
    /// A finger lifted: the pointer goes away at the start of the next frame, not in the one
    /// with the release, or egui would lose the position it needs to see a tap as a click.
    pointer_gone_next: bool,
    /// Whether Cmd is egui's `command` (Apple platforms).
    command_is_gui: bool,
}

impl Default for EguiInput {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            modifiers: egui::Modifiers::NONE,
            points_per_unit: 1.0,
            window_size: Vec2::new(1.0, 1.0),
            pointer_finger: None,
            pointer_gone_next: false,
            command_is_gui: matches!(Os::CURRENT, Os::MacOs | Os::Ios),
        }
    }
}

impl EguiInput {
    /// Uses `metrics` at `pixels_per_point` for the events that follow.
    pub fn set_metrics(&mut self, metrics: &WindowMetrics, pixels_per_point: f32) {
        self.points_per_unit = metrics.pixel_density() / pixels_per_point.max(f32::EPSILON);
        self.window_size = Vec2::new(metrics.size.width as f32, metrics.size.height as f32);
    }

    /// The queued events, for the frame's `RawInput`.
    pub fn take_events(&mut self) -> Vec<egui::Event> {
        let events = std::mem::take(&mut self.events);
        if std::mem::take(&mut self.pointer_gone_next) {
            self.events.push(egui::Event::PointerGone);
        }
        events
    }

    /// A position in window coordinates, in points.
    pub fn to_points(&self, x: f32, y: f32) -> Pos2 {
        Pos2::new(x * self.points_per_unit, y * self.points_per_unit)
    }

    /// A point in window coordinates: the inverse of [`Self::to_points`].
    pub fn to_window(&self, point: Pos2) -> (f32, f32) {
        let units_per_point = 1.0 / self.points_per_unit.max(f32::EPSILON);
        (point.x * units_per_point, point.y * units_per_point)
    }

    /// A normalized touch position, in points.
    pub fn touch_to_points(&self, x: f32, y: f32) -> Pos2 {
        self.to_points(x * self.window_size.x, y * self.window_size.y)
    }

    /// Translates an event the router gave the overlay. `finger` is the touch's slot.
    pub fn push(&mut self, event: &Event, finger: Option<usize>) {
        match *event {
            Event::Key(key) => self.key(&key),
            Event::Text(text) => {
                let text: String = text.text().chars().filter(|&c| is_printable(c)).collect();
                if !text.is_empty() {
                    self.events.push(egui::Event::Text(text));
                }
            }
            Event::MouseMotion(motion) => {
                self.events.push(egui::Event::PointerMoved(self.to_points(motion.x, motion.y)))
            }
            Event::MouseButton(button) => {
                let Some(pointer) = pointer_button(button.button) else { return };
                self.events.push(egui::Event::PointerButton {
                    pos: self.to_points(button.x, button.y),
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
            Event::Touch(touch) => {
                if let Some(finger) = finger {
                    self.touch(&touch, finger);
                }
            }
            Event::Window { event: WindowEvent::FocusLost, .. } => self.release_all(),
            Event::Window { event: WindowEvent::FocusGained, .. } => {
                self.events.push(egui::Event::WindowFocused(true));
            }
            _ => {}
        }
    }

    /// The window lost focus or the overlay hid: nothing stays pressed or hovered.
    pub fn release_all(&mut self) {
        self.modifiers = egui::Modifiers::NONE;
        self.pointer_finger = None;
        self.pointer_gone_next = false;
        self.events.push(egui::Event::WindowFocused(false));
        self.events.push(egui::Event::PointerGone);
    }

    /// A touch the router took away from the overlay (the toggle gesture claimed it).
    pub fn cancel_touch(&mut self, touch: &TouchEvent, finger: usize) {
        self.touch(&TouchEvent { phase: TouchPhase::Canceled, ..*touch }, finger);
    }

    fn key(&mut self, key: &KeyEvent) {
        self.modifiers = modifiers(key.modifiers, self.command_is_gui);
        let physical = key_from_scancode(key.scancode);
        let Some(logical) = logical_key(key.keycode, key.scancode) else {
            // Modifier keys carry their news in `modifiers`.
            self.events.push(egui::Event::ModifiersChanged(self.modifiers));
            return;
        };
        self.events.push(egui::Event::Key {
            key: logical,
            physical_key: physical,
            pressed: key.down,
            repeat: key.repeat,
            modifiers: self.modifiers,
        });
    }

    fn touch(&mut self, touch: &TouchEvent, finger: usize) {
        let pos = self.touch_to_points(touch.x, touch.y);
        let phase = match touch.phase {
            TouchPhase::Down => egui::TouchPhase::Start,
            TouchPhase::Moved => egui::TouchPhase::Move,
            TouchPhase::Up => egui::TouchPhase::End,
            TouchPhase::Canceled => egui::TouchPhase::Cancel,
        };
        self.events.push(egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(finger as u64),
            phase,
            pos,
            force: Some(touch.pressure),
        });
        // The first finger down is egui's pointer until it lifts.
        if touch.phase == TouchPhase::Down && self.pointer_finger.is_none() {
            self.pointer_finger = Some(finger);
        }
        if self.pointer_finger != Some(finger) {
            return;
        }
        match touch.phase {
            TouchPhase::Down => {
                self.events.push(egui::Event::PointerMoved(pos));
                self.events.push(egui::Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: self.modifiers,
                });
            }
            TouchPhase::Moved => self.events.push(egui::Event::PointerMoved(pos)),
            TouchPhase::Up => {
                self.events.push(egui::Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers: self.modifiers,
                });
                self.pointer_gone_next = true;
                self.pointer_finger = None;
            }
            TouchPhase::Canceled => {
                self.events.push(egui::Event::PointerGone);
                self.pointer_finger = None;
            }
        }
    }
}

/// Characters egui should insert: not control characters, and not the private-use range macOS
/// uses for function keys.
fn is_printable(c: char) -> bool {
    !c.is_control() && !('\u{e000}'..='\u{f8ff}').contains(&c)
}

fn modifiers(mods: Modifiers, command_is_gui: bool) -> egui::Modifiers {
    egui::Modifiers {
        alt: mods.alt,
        ctrl: mods.ctrl,
        shift: mods.shift,
        mac_cmd: command_is_gui && mods.gui,
        command: if command_is_gui { mods.gui } else { mods.ctrl },
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

/// The key as the layout names it: the character it types, or for keys that type none (arrows,
/// Enter, F-keys), the key in that position.
fn logical_key(keycode: Keycode, scancode: Scancode) -> Option<Key> {
    keycode.to_char().and_then(key_from_char).or_else(|| key_from_scancode(scancode))
}

fn key_from_char(c: char) -> Option<Key> {
    Some(match c.to_ascii_lowercase() {
        c @ 'a'..='z' => [
            Key::A,
            Key::B,
            Key::C,
            Key::D,
            Key::E,
            Key::F,
            Key::G,
            Key::H,
            Key::I,
            Key::J,
            Key::K,
            Key::L,
            Key::M,
            Key::N,
            Key::O,
            Key::P,
            Key::Q,
            Key::R,
            Key::S,
            Key::T,
            Key::U,
            Key::V,
            Key::W,
            Key::X,
            Key::Y,
            Key::Z,
        ][usize::from(c as u8 - b'a')],
        c @ '0'..='9' => [
            Key::Num0,
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ][usize::from(c as u8 - b'0')],
        ' ' => Key::Space,
        '-' => Key::Minus,
        '=' => Key::Equals,
        '+' => Key::Plus,
        '[' => Key::OpenBracket,
        ']' => Key::CloseBracket,
        '{' => Key::OpenCurlyBracket,
        '}' => Key::CloseCurlyBracket,
        '\\' => Key::Backslash,
        '/' => Key::Slash,
        '|' => Key::Pipe,
        '?' => Key::Questionmark,
        '!' => Key::Exclamationmark,
        ';' => Key::Semicolon,
        ':' => Key::Colon,
        '\'' => Key::Quote,
        ',' => Key::Comma,
        '.' => Key::Period,
        '`' => Key::Backtick,
        _ => return None,
    })
}

/// A key by its position (USB HID usage, as SDL numbers scancodes). Modifier keys map to
/// nothing: egui hears about them through [`egui::Modifiers`].
fn key_from_scancode(scancode: Scancode) -> Option<Key> {
    const LETTERS: [Key; 26] = [
        Key::A,
        Key::B,
        Key::C,
        Key::D,
        Key::E,
        Key::F,
        Key::G,
        Key::H,
        Key::I,
        Key::J,
        Key::K,
        Key::L,
        Key::M,
        Key::N,
        Key::O,
        Key::P,
        Key::Q,
        Key::R,
        Key::S,
        Key::T,
        Key::U,
        Key::V,
        Key::W,
        Key::X,
        Key::Y,
        Key::Z,
    ];
    // Scancodes 30..=39 are 1..9 then 0.
    const DIGITS: [Key; 10] = [
        Key::Num1,
        Key::Num2,
        Key::Num3,
        Key::Num4,
        Key::Num5,
        Key::Num6,
        Key::Num7,
        Key::Num8,
        Key::Num9,
        Key::Num0,
    ];
    const F_KEYS: [Key; 12] = [
        Key::F1,
        Key::F2,
        Key::F3,
        Key::F4,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::F9,
        Key::F10,
        Key::F11,
        Key::F12,
    ];
    let code = scancode.0;
    Some(match code {
        4..=29 => LETTERS[usize::from(code - 4)],
        30..=39 => DIGITS[usize::from(code - 30)],
        58..=69 => F_KEYS[usize::from(code - 58)],
        _ => match scancode {
            Scancode::RETURN => Key::Enter,
            Scancode::ESCAPE => Key::Escape,
            Scancode::BACKSPACE => Key::Backspace,
            Scancode::TAB => Key::Tab,
            Scancode::SPACE => Key::Space,
            Scancode::MINUS => Key::Minus,
            Scancode::EQUALS => Key::Equals,
            Scancode::LEFT_BRACKET => Key::OpenBracket,
            Scancode::RIGHT_BRACKET => Key::CloseBracket,
            Scancode::BACKSLASH => Key::Backslash,
            Scancode::SEMICOLON => Key::Semicolon,
            Scancode::APOSTROPHE => Key::Quote,
            Scancode::GRAVE => Key::Backtick,
            Scancode::COMMA => Key::Comma,
            Scancode::PERIOD => Key::Period,
            Scancode::SLASH => Key::Slash,
            Scancode::INSERT => Key::Insert,
            Scancode::HOME => Key::Home,
            Scancode::PAGE_UP => Key::PageUp,
            Scancode::DELETE => Key::Delete,
            Scancode::END => Key::End,
            Scancode::PAGE_DOWN => Key::PageDown,
            Scancode::RIGHT => Key::ArrowRight,
            Scancode::LEFT => Key::ArrowLeft,
            Scancode::DOWN => Key::ArrowDown,
            Scancode::UP => Key::ArrowUp,
            _ => return None,
        },
    })
}

#[cfg(test)]
mod tests {
    use sapphire_platform::{
        FingerId, MouseButtonEvent, MouseMotionEvent, MouseWheelEvent, TextEvent, Timestamp, TouchDeviceId,
        WindowId,
    };

    use super::*;

    const WINDOW: WindowId = WindowId::from_raw(1);
    const AT: Timestamp = Timestamp::from_nanos(1);

    /// A 2× Retina window, 800 × 600 in window coordinates, at zoom 1.
    fn input() -> EguiInput {
        let mut input = EguiInput { command_is_gui: true, ..EguiInput::default() };
        let metrics = WindowMetrics {
            pixel_size: Size { width: 1600, height: 1200 },
            size: Size { width: 800, height: 600 },
            display_scale: 2.0,
            safe_area: Rect { x: 0, y: 0, width: 800, height: 600 },
            focused: true,
        };
        input.set_metrics(&metrics, 2.0);
        input
    }

    fn key(scancode: Scancode, keycode: u32, down: bool, modifiers: Modifiers) -> Event {
        Event::Key(KeyEvent {
            timestamp: AT,
            window: WINDOW,
            scancode,
            keycode: Keycode(keycode),
            modifiers,
            down,
            repeat: false,
        })
    }

    #[test]
    fn keys_are_logical_with_physical_positions() {
        let mut input = input();
        // AZERTY: the key in QWERTY's Q position types `a`.
        input.push(&key(Scancode::Q, u32::from('a'), true, Modifiers::default()), None);
        // Enter has no character; the scancode names it.
        input.push(&key(Scancode::RETURN, (1 << 30) | 40, false, Modifiers::default()), None);
        let events = input.take_events();
        assert!(matches!(
            events[0],
            egui::Event::Key { key: Key::A, physical_key: Some(Key::Q), pressed: true, repeat: false, .. }
        ));
        assert!(matches!(events[1], egui::Event::Key { key: Key::Enter, pressed: false, .. }));
        assert!(input.take_events().is_empty(), "taken once");

        assert_eq!(key_from_scancode(Scancode::DIGIT_0), Some(Key::Num0));
        assert_eq!(key_from_scancode(Scancode::F12), Some(Key::F12));
        assert_eq!(key_from_scancode(Scancode::LEFT_SHIFT), None);
        assert_eq!(key_from_char('Z'), Some(Key::Z));
        assert_eq!(key_from_char('é'), None);
    }

    #[test]
    fn modifiers_map_command_per_platform() {
        let mods = Modifiers { shift: true, ctrl: false, alt: true, gui: true };
        let mac = modifiers(mods, true);
        assert!(mac.shift && mac.alt && mac.mac_cmd && mac.command && !mac.ctrl);
        let other = modifiers(Modifiers { ctrl: true, gui: false, ..mods }, false);
        assert!(other.command && other.ctrl && !other.mac_cmd);

        let mut input = input();
        input.push(
            &key(Scancode::LEFT_GUI, (1 << 30) | 227, true, Modifiers { gui: true, ..Default::default() }),
            None,
        );
        input.push(
            &key(Scancode::C, u32::from('c'), true, Modifiers { gui: true, ..Default::default() }),
            None,
        );
        let events = input.take_events();
        assert!(matches!(events[0], egui::Event::ModifiersChanged(m) if m.command));
        assert!(matches!(events[1], egui::Event::Key { key: Key::C, modifiers, .. } if modifiers.command));
        assert!(input.modifiers.mac_cmd);
    }

    #[test]
    fn pointer_and_wheel_are_in_points() {
        let mut input = input();
        let motion = MouseMotionEvent { timestamp: AT, window: WINDOW, x: 100.0, y: 50.0, dx: 1.0, dy: 0.0 };
        input.push(&Event::MouseMotion(motion), None);
        let button = MouseButtonEvent {
            timestamp: AT,
            window: WINDOW,
            button: MouseButton::Right,
            down: true,
            clicks: 1,
            x: 100.0,
            y: 50.0,
        };
        input.push(&Event::MouseButton(button), None);
        input.push(&Event::MouseButton(MouseButtonEvent { button: MouseButton::Other(9), ..button }), None);
        input.push(
            &Event::MouseWheel(MouseWheelEvent { timestamp: AT, window: WINDOW, x: 0.0, y: -2.0 }),
            None,
        );
        let events = input.take_events();
        assert_eq!(events.len(), 3, "unknown buttons are dropped");
        // 2 pixels per window unit on a Retina display, 2 pixels per point: 1 point per unit.
        assert_eq!(events[0], egui::Event::PointerMoved(Pos2::new(100.0, 50.0)));
        assert!(matches!(
            events[1],
            egui::Event::PointerButton { button: PointerButton::Secondary, pressed: true, pos, .. }
                if pos == Pos2::new(100.0, 50.0)
        ));
        assert!(matches!(events[2], egui::Event::MouseWheel { delta, unit: egui::MouseWheelUnit::Line, .. }
            if delta == Vec2::new(0.0, -2.0)));

        // Zooming the overlay to 2× halves every position.
        let mut zoomed = input;
        zoomed.set_metrics(&WindowMetrics::offscreen(1600, 1200, 2.0), 4.0);
        assert_eq!(zoomed.to_points(100.0, 50.0), Pos2::new(25.0, 12.5));
    }

    #[test]
    fn the_first_finger_drives_the_pointer() {
        let mut input = input();
        let finger = FingerId::new(TouchDeviceId::from_raw(1), 7);
        let touch = |phase, x| {
            Event::Touch(TouchEvent {
                timestamp: AT,
                window: WINDOW,
                finger,
                phase,
                x,
                y: 0.5,
                pressure: 1.0,
            })
        };
        input.push(&touch(TouchPhase::Down, 0.25), Some(0));
        input.push(&touch(TouchPhase::Down, 0.75), Some(1));
        input.push(&touch(TouchPhase::Moved, 0.5), Some(0));
        input.push(&touch(TouchPhase::Up, 0.5), Some(0));
        input.push(&touch(TouchPhase::Down, 0.5), None);
        let mut events = input.take_events();
        // The lift's pointer-gone waits for the next frame, so a tap within one frame clicks.
        assert!(!events.contains(&egui::Event::PointerGone));
        let next = input.take_events();
        assert_eq!(next, [egui::Event::PointerGone]);
        events.extend(next);
        let pointer: Vec<&egui::Event> =
            events.iter().filter(|event| !matches!(event, egui::Event::Touch { .. })).collect();
        // Normalized 0.25 of 800 units is 200 points.
        assert_eq!(pointer[0], &egui::Event::PointerMoved(Pos2::new(200.0, 300.0)));
        assert!(matches!(pointer[1], egui::Event::PointerButton { pressed: true, .. }));
        assert_eq!(pointer[2], &egui::Event::PointerMoved(Pos2::new(400.0, 300.0)), "only finger 0 moves it");
        assert!(matches!(pointer[3], egui::Event::PointerButton { pressed: false, .. }));
        assert_eq!(pointer[4], &egui::Event::PointerGone);
        assert_eq!(pointer.len(), 5);
        let touches = events.iter().filter(|event| matches!(event, egui::Event::Touch { .. })).count();
        assert_eq!(touches, 4, "a touch the router gave nobody is dropped");
    }

    #[test]
    fn text_and_focus() {
        let mut input = input();
        for text in TextEvent::split(AT, WINDOW, "ok\u{7f}\u{f700}é") {
            input.push(&Event::Text(text), None);
        }
        input.push(&Event::Window { timestamp: AT, window: WINDOW, event: WindowEvent::FocusLost }, None);
        let events = input.take_events();
        assert_eq!(
            events[0],
            egui::Event::Text("oké".into()),
            "control and private-use characters are dropped"
        );
        assert_eq!(&events[1..], [egui::Event::WindowFocused(false), egui::Event::PointerGone]);
        assert_eq!(input.modifiers, egui::Modifiers::NONE);
    }
}
