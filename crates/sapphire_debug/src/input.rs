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
//! - **Letting go without a click.** When the pointer's finger is cancelled, the window loses
//!   focus or the foreground, or the overlay hides or shows, egui's pointer is reset with nothing
//!   held or dragged (`reset_pointer`), and the overlay's touches end as cancelled. Never by a
//!   release, which would click the widget under the press.
//!
//! Not translated yet: clipboard copy and paste (the platform has no clipboard API yet), IME
//! composition previews, and cursor shapes.

use std::collections::VecDeque;

use egui::{Key, PointerButton, Pos2, Vec2};
use sapphire_input::MAX_FINGERS;
use sapphire_platform::{
    Event, KeyEvent, Keycode, LifecycleEvent, Modifiers, MouseButton, Os, Rect, Scancode, Size, TouchEvent,
    TouchPhase, Window, WindowEvent,
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
    /// The events, in runs, each for a pass of its own ([`Self::run_pass`]): every run but the
    /// last ends in a pointer reset, and the last may. Almost always one run.
    runs: VecDeque<Run>,
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
    /// The touches egui will hold once the queued events reach it, by finger slot, where each
    /// was last: what [`Self::release_all`] cancels.
    touches: [Option<Pos2>; MAX_FINGERS],
    /// Whether Cmd is egui's `command` (Apple platforms).
    command_is_gui: bool,
}

/// Events for one pass, and whether egui's pointer is reset after it.
#[derive(Debug, Default)]
struct Run {
    events: Vec<egui::Event>,
    reset_after: bool,
}

impl Default for EguiInput {
    fn default() -> Self {
        Self {
            runs: VecDeque::from([Run::default()]),
            modifiers: egui::Modifiers::NONE,
            points_per_unit: 1.0,
            window_size: Vec2::new(1.0, 1.0),
            pointer_finger: None,
            pointer_gone_next: false,
            touches: [None; MAX_FINGERS],
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

    /// Runs a pass of `ctx` on `raw` with the queued events, and resets egui's pointer after it
    /// if they end in a reset ([`reset_pointer`]). Events queued after a reset wait for the next
    /// pass, so the reset forgets only the presses before it.
    pub fn run_pass(
        &mut self,
        ctx: &egui::Context,
        mut raw: egui::RawInput,
        ui: impl FnMut(&egui::Context),
    ) -> egui::FullOutput {
        let reset = self.take_events(&mut raw.events);
        let output = ctx.run_pass(raw, ui);
        if reset {
            reset_pointer(ctx);
        }
        output
    }

    /// The overlay hid or showed: drops what it had queued but its touches' events (which click
    /// nothing, and end touches egui holds), releases everything, and resets egui's pointer now.
    pub fn restart(&mut self, ctx: &egui::Context) {
        let touches = self
            .runs
            .drain(..)
            .flat_map(|run| run.events)
            .filter(|event| matches!(event, egui::Event::Touch { .. }))
            .collect();
        self.runs.push_back(Run { events: touches, reset_after: false });
        self.release_all();
        reset_pointer(ctx);
    }

    /// Moves the next pass's events to `events`; `true` if egui's pointer is reset after it.
    fn take_events(&mut self, events: &mut Vec<egui::Event>) -> bool {
        let run = self.runs.pop_front().unwrap_or_default();
        *events = run.events;
        // A lift since the last reset is in the last run: its pointer-gone starts the pass after.
        if self.runs.is_empty() {
            self.runs.push_back(Run::default());
            if std::mem::take(&mut self.pointer_gone_next) {
                self.queue(egui::Event::PointerGone);
            }
        }
        run.reset_after
    }

    /// Queues an event, after the last reset.
    fn queue(&mut self, event: egui::Event) {
        match self.runs.back_mut() {
            Some(run) if !run.reset_after => run.events.push(event),
            _ => self.runs.push_back(Run { events: vec![event], reset_after: false }),
        }
    }

    /// Ends the queued events with a pointer reset, which also forgets where a lifted finger was.
    fn queue_reset(&mut self) {
        self.pointer_gone_next = false;
        match self.runs.back_mut() {
            Some(run) => run.reset_after = true,
            None => self.runs.push_back(Run { events: Vec::new(), reset_after: true }),
        }
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
                    self.queue(egui::Event::Text(text));
                }
            }
            Event::MouseMotion(motion) => {
                self.queue(egui::Event::PointerMoved(self.to_points(motion.x, motion.y)))
            }
            Event::MouseButton(button) => {
                let Some(pointer) = pointer_button(button.button) else { return };
                self.queue(egui::Event::PointerButton {
                    pos: self.to_points(button.x, button.y),
                    button: pointer,
                    pressed: button.down,
                    modifiers: self.modifiers,
                });
            }
            Event::MouseWheel(wheel) => self.queue(egui::Event::MouseWheel {
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
            // The router forgets every press on both; so does egui.
            Event::Window { event: WindowEvent::FocusLost, .. }
            | Event::Lifecycle { event: LifecycleEvent::WillEnterBackground, .. } => self.release_all(),
            Event::Window { event: WindowEvent::FocusGained, .. } => {
                self.queue(egui::Event::WindowFocused(true));
            }
            _ => {}
        }
    }

    /// The window lost focus or the foreground, or the overlay hid or showed: nothing stays
    /// pressed, touched or hovered. The overlay's touches end as cancelled, and egui's pointer
    /// is reset after the pass that takes these events, with no release, so nothing is clicked.
    pub fn release_all(&mut self) {
        self.modifiers = egui::Modifiers::NONE;
        self.pointer_finger = None;
        let touches = std::mem::replace(&mut self.touches, [None; MAX_FINGERS]);
        for (slot, pos) in touches.into_iter().enumerate() {
            if let Some(pos) = pos {
                self.queue(egui::Event::Touch {
                    device_id: egui::TouchDeviceId(0),
                    id: egui::TouchId(slot as u64),
                    phase: egui::TouchPhase::Cancel,
                    pos,
                    force: None,
                });
            }
        }
        self.queue(egui::Event::WindowFocused(false));
        self.queue(egui::Event::PointerGone);
        self.queue_reset();
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
            self.queue(egui::Event::ModifiersChanged(self.modifiers));
            return;
        };
        self.queue(egui::Event::Key {
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
        self.queue(egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(finger as u64),
            phase,
            pos,
            force: Some(touch.pressure),
        });
        if let Some(held) = self.touches.get_mut(finger) {
            *held = match touch.phase {
                TouchPhase::Down => Some(pos),
                TouchPhase::Moved => held.map(|_| pos),
                TouchPhase::Up | TouchPhase::Canceled => None,
            };
        }
        // The first finger down is egui's pointer until it lifts. Its position replaces a lifted
        // finger's whose pointer-gone hasn't gone out yet: that would lose it.
        if touch.phase == TouchPhase::Down && self.pointer_finger.is_none() {
            self.pointer_finger = Some(finger);
            self.pointer_gone_next = false;
        }
        if self.pointer_finger != Some(finger) {
            return;
        }
        match touch.phase {
            TouchPhase::Down => {
                self.queue(egui::Event::PointerMoved(pos));
                self.queue(egui::Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: self.modifiers,
                });
            }
            TouchPhase::Moved => self.queue(egui::Event::PointerMoved(pos)),
            TouchPhase::Up => {
                self.queue(egui::Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers: self.modifiers,
                });
                self.pointer_gone_next = true;
                self.pointer_finger = None;
            }
            // `PointerGone` keeps egui's button down (drags may leave the window): the reset
            // after this pass lets it go without a release, which would click.
            TouchPhase::Canceled => {
                self.queue(egui::Event::PointerGone);
                self.queue_reset();
                self.pointer_finger = None;
            }
        }
    }
}

/// Lets go of egui's pointer between passes: no button down and nothing dragged, without the
/// release that would click the widget under the press (or, sent elsewhere, drag a slider
/// there).
///
/// egui 0.37 has no event for it: `PointerGone` keeps buttons down so drags may leave the
/// window, and `WindowFocused(false)` clears only keys and modifiers. So this replaces the
/// context's `PointerState` with its default through `Context::input_mut_for` and ends any drag
/// with `Context::stop_dragging`, both public in egui 0.37.0. The widget a press would click
/// (egui's private `potential_click_id`) egui clears itself in a pass whose events end with the
/// pointer gone, as a reset's do. A release egui then sees is no click, since it saw no press.
/// The reset forgets every press before it, so one after it in the same frame waits for the
/// next pass ([`EguiInput::run_pass`]). Recheck all of this on any egui upgrade; this module's
/// tests hold it.
pub(crate) fn reset_pointer(ctx: &egui::Context) {
    ctx.input_mut_for(egui::ViewportId::ROOT, |input| input.pointer = egui::PointerState::default());
    ctx.stop_dragging();
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

    /// The next pass's events.
    fn take(input: &mut EguiInput) -> Vec<egui::Event> {
        let mut events = Vec::new();
        input.take_events(&mut events);
        events
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
        let events = take(&mut input);
        assert!(matches!(
            events[0],
            egui::Event::Key { key: Key::A, physical_key: Some(Key::Q), pressed: true, repeat: false, .. }
        ));
        assert!(matches!(events[1], egui::Event::Key { key: Key::Enter, pressed: false, .. }));
        assert!(take(&mut input).is_empty(), "taken once");

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
        let events = take(&mut input);
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
        let events = take(&mut input);
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
        let mut events = take(&mut input);
        // The lift's pointer-gone waits for the next frame, so a tap within one frame clicks.
        assert!(!events.contains(&egui::Event::PointerGone));
        let next = take(&mut input);
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
        let events = take(&mut input);
        assert_eq!(
            events[0],
            egui::Event::Text("oké".into()),
            "control and private-use characters are dropped"
        );
        assert_eq!(&events[1..], [egui::Event::WindowFocused(false), egui::Event::PointerGone]);
        assert_eq!(input.modifiers, egui::Modifiers::NONE);
    }

    /// egui with a button and a slider under it, driven through [`EguiInput`] as the overlay
    /// drives it, in the 800 × 600 window of [`input`], whose window coordinates are points.
    struct Ui {
        ctx: egui::Context,
        input: EguiInput,
        clicks: u32,
        value: f32,
        button: egui::Rect,
        slider: egui::Rect,
    }

    impl Ui {
        fn new() -> Self {
            let mut ui = Self {
                ctx: egui::Context::default(),
                input: input(),
                clicks: 0,
                value: 0.0,
                button: egui::Rect::NOTHING,
                slider: egui::Rect::NOTHING,
            };
            // egui hit-tests what the last pass drew, and a new area takes input from its second.
            ui.frame();
            ui.frame();
            ui
        }

        fn frame(&mut self) {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
                ..egui::RawInput::default()
            };
            let Self { ctx, input, clicks, value, button, slider } = self;
            let mut output = input.run_pass(ctx, raw, |ctx| {
                egui::Area::new(egui::Id::unique("sapphire_debug.input_test")).fixed_pos(Pos2::ZERO).show(
                    ctx,
                    |ui| {
                        let response = ui.button("A button");
                        *clicks += u32::from(response.clicked());
                        *button = response.rect;
                        *slider = ui.add(egui::Slider::new(value, 0.0..=100.0)).rect;
                    },
                );
            });
            output.textures_delta.clear();
        }

        fn push(&mut self, event: &Event, finger: Option<usize>) {
            self.input.push(event, finger);
        }

        fn down(&self) -> bool {
            self.ctx.input(|input| input.pointer.any_down())
        }
    }

    fn touch_at(finger: u64, phase: TouchPhase, at: Pos2) -> Event {
        Event::Touch(TouchEvent {
            timestamp: AT,
            window: WINDOW,
            finger: FingerId::new(TouchDeviceId::from_raw(1), finger),
            phase,
            x: at.x / 800.0,
            y: at.y / 600.0,
            pressure: 1.0,
        })
    }

    fn mouse_button(down: bool, at: Pos2) -> Event {
        Event::MouseButton(MouseButtonEvent {
            timestamp: AT,
            window: WINDOW,
            button: MouseButton::Left,
            down,
            clicks: 1,
            x: at.x,
            y: at.y,
        })
    }

    fn mouse_motion(at: Pos2) -> Event {
        Event::MouseMotion(MouseMotionEvent {
            timestamp: AT,
            window: WINDOW,
            x: at.x,
            y: at.y,
            dx: 0.0,
            dy: 0.0,
        })
    }

    const AWAY: Pos2 = Pos2::new(600.0, 400.0);

    #[test]
    fn a_finger_taking_over_the_pointer_in_the_lift_frame_still_taps() {
        // Found in the review-overlay batch: the finger driving egui's pointer lifts and another
        // lands in the same frame. The lift's pointer-gone opened the next pass while the new
        // finger was down, so egui lost its position and the widget its press would click.
        let mut ui = Ui::new();
        let at = ui.button.center();
        ui.push(&touch_at(1, TouchPhase::Down, AWAY), Some(0));
        ui.frame();
        ui.push(&touch_at(1, TouchPhase::Up, AWAY), Some(0));
        ui.push(&touch_at(2, TouchPhase::Down, at), Some(1));
        ui.frame();
        // The new finger holds still for a frame, then lifts.
        ui.frame();
        assert!(ui.down(), "the new finger is still down");
        assert!(ui.ctx.input(|input| input.pointer.latest_pos()).is_some(), "where it is");
        ui.push(&touch_at(2, TouchPhase::Up, at), Some(1));
        ui.frame();
        assert_eq!((ui.clicks, ui.down()), (1, false), "the new finger taps");
        // Lifted with no finger after it, the pointer still goes in the next pass.
        ui.frame();
        assert!(ui.ctx.input(|input| input.pointer.latest_pos()).is_none());
    }

    #[test]
    fn taps_and_clicks_still_click() {
        let mut ui = Ui::new();
        let at = ui.button.center();
        // A tap within one frame, and across frames.
        ui.push(&touch_at(1, TouchPhase::Down, at), Some(0));
        ui.push(&touch_at(1, TouchPhase::Up, at), Some(0));
        ui.frame();
        assert_eq!(ui.clicks, 1);
        ui.push(&touch_at(1, TouchPhase::Down, at), Some(0));
        ui.frame();
        ui.push(&touch_at(1, TouchPhase::Up, at), Some(0));
        ui.frame();
        assert_eq!(ui.clicks, 2);
        ui.push(&mouse_button(true, at), None);
        ui.frame();
        ui.push(&mouse_button(false, at), None);
        ui.frame();
        ui.frame();
        assert_eq!((ui.clicks, ui.down()), (3, false));
    }

    #[test]
    fn a_cancelled_touch_lets_go_without_a_click() {
        // DT-003: `PointerGone` alone left egui's button down.
        let mut ui = Ui::new();
        let at = ui.button.center();
        ui.push(&touch_at(1, TouchPhase::Down, at), Some(0));
        ui.frame();
        assert!(ui.down() && ui.ctx.input(|input| input.any_touches()));
        ui.push(&touch_at(1, TouchPhase::Canceled, at), Some(0));
        ui.frame();
        assert!(!ui.down(), "a cancel lets go");
        assert!(!ui.ctx.input(|input| input.any_touches()));
        ui.frame();
        assert_eq!(ui.clicks, 0);

        // A cancel, and another finger pressing elsewhere in the same frame and lifting in the
        // next: nothing is clicked.
        ui.push(&touch_at(1, TouchPhase::Down, at), Some(0));
        ui.frame();
        ui.push(&touch_at(1, TouchPhase::Canceled, at), Some(0));
        ui.push(&touch_at(2, TouchPhase::Down, AWAY), Some(1));
        ui.frame();
        assert!(!ui.down(), "the cancel's pass lets go");
        ui.push(&touch_at(2, TouchPhase::Up, AWAY), Some(1));
        ui.frame();
        ui.frame();
        assert_eq!((ui.clicks, ui.down()), (0, false), "the cancelled press never clicks");

        // Nor is the other finger's press lost: it reaches egui in a pass after the reset, which
        // would otherwise forget it too, and taps.
        ui.push(&touch_at(1, TouchPhase::Down, at), Some(0));
        ui.frame();
        ui.push(&touch_at(1, TouchPhase::Canceled, at), Some(0));
        ui.push(&touch_at(2, TouchPhase::Down, at), Some(1));
        ui.frame();
        ui.frame();
        assert!(ui.down(), "the next finger's press, a pass later");
        ui.push(&touch_at(2, TouchPhase::Up, at), Some(1));
        ui.frame();
        assert_eq!((ui.clicks, ui.down()), (1, false), "the next finger taps");
    }

    #[test]
    fn losing_focus_or_the_foreground_lets_go_without_a_click() {
        for lost in [
            Event::Window { timestamp: AT, window: WINDOW, event: WindowEvent::FocusLost },
            Event::Lifecycle { timestamp: AT, event: LifecycleEvent::WillEnterBackground },
        ] {
            let mut ui = Ui::new();
            let at = ui.button.center();
            ui.push(&mouse_button(true, at), None);
            ui.frame();
            assert!(ui.down());
            ui.push(&lost, None);
            ui.frame();
            assert!(!ui.down(), "{lost:?} lets go");
            // The router forgot the press, so its release reaches both sides.
            ui.push(&mouse_button(false, at), None);
            ui.frame();
            assert_eq!(ui.clicks, 0, "{lost:?}");

            // The overlay's fingers are cancelled too.
            ui.push(&touch_at(1, TouchPhase::Down, AWAY), Some(0));
            ui.push(&touch_at(2, TouchPhase::Down, AWAY), Some(1));
            ui.frame();
            assert!(ui.ctx.input(|input| input.any_touches()));
            ui.push(&lost, None);
            ui.frame();
            assert!(!ui.down() && !ui.ctx.input(|input| input.any_touches()), "{lost:?}");
        }
    }

    #[test]
    fn hiding_and_showing_ends_a_drag() {
        let mut ui = Ui::new();
        let start = ui.slider.left_center() + Vec2::new(10.0, 0.0);
        ui.push(&mouse_motion(start), None);
        ui.push(&mouse_button(true, start), None);
        ui.frame();
        ui.push(&mouse_motion(start + Vec2::new(40.0, 0.0)), None);
        ui.frame();
        let dragged = ui.value;
        assert!(dragged > 0.0 && ui.ctx.dragged_id().is_some(), "{dragged}");
        // The overlay hides with the button held, and shows again later.
        ui.input.restart(&ui.ctx);
        ui.input.restart(&ui.ctx);
        ui.frame();
        ui.push(&mouse_motion(start + Vec2::new(80.0, 0.0)), None);
        ui.frame();
        ui.frame();
        assert_eq!(ui.value, dragged, "the slider no longer follows the pointer");
        assert!(!ui.down() && ui.ctx.dragged_id().is_none());
        assert_eq!(ui.clicks, 0);

        // A finger held on the overlay when it hides ends in egui too.
        ui.push(&touch_at(1, TouchPhase::Down, AWAY), Some(0));
        ui.frame();
        ui.input.restart(&ui.ctx);
        ui.input.restart(&ui.ctx);
        ui.frame();
        assert!(!ui.down() && !ui.ctx.input(|input| input.any_touches()));
    }
}
