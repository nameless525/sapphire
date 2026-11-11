//! The player's bindings (M4 A4): what they've remapped, and which held actions they toggle, laid over a
//! game's bindings document ([`layered`]), with what a remap screen asks of them on the way: whether a press
//! is another binding's ([`taken`]), and what a prompt shows for a source ([`glyph`]).
//!
//! A player's file holds only what differs from the document ([`set`] drops a remap back to it), so a
//! game that changes a default carries every player who never touched that binding along with it.

use std::borrow::Cow;

use sapphire_input::{
    CaptureFrom, GamepadFamily, InputBindings, Key, MouseButton, PadAxis, PadButton, Source, Stick, Trigger,
};
use serde::{Deserialize, Serialize};

/// A device whose bindings a player remaps. A binding is the device's when its name ends in the device's
/// [`suffix`](Self::suffix), as `light.gamepad` and `light.keyboard` do in `combat_bindings.ron`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RemapDevice {
    Gamepad,
    Keyboard,
}

impl RemapDevice {
    pub const ALL: [Self; 2] = [Self::Gamepad, Self::Keyboard];

    /// Its name, as keys end in it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Gamepad => "gamepad",
            Self::Keyboard => "keyboard",
        }
    }

    /// What the names of its bindings end in.
    #[must_use]
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::Gamepad => ".gamepad",
            Self::Keyboard => ".keyboard",
        }
    }

    /// The device whose binding `name` is, by its suffix.
    #[must_use]
    pub fn of_binding(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|device| name.ends_with(device.suffix()))
    }

    /// What a capture for one of its bindings takes input from.
    #[must_use]
    pub const fn capture(self) -> CaptureFrom {
        match self {
            Self::Gamepad => CaptureFrom::Gamepad,
            Self::Keyboard => CaptureFrom::Keyboard,
        }
    }

    /// Whether one of its bindings can read `source`: a pad's button or trigger, or a key. A stick isn't
    /// a button, and moves and looks.
    #[must_use]
    pub fn accepts(self, source: &Source) -> bool {
        match (self, source) {
            (Self::Gamepad, Source::Pad(_)) | (Self::Keyboard, Source::Key(_)) => true,
            (Self::Gamepad, Source::PadAxis(axis)) => {
                matches!(axis, PadAxis::LeftTrigger | PadAxis::RightTrigger)
            }
            _ => false,
        }
    }

    /// The press that ends a capture on it rather than being taken: Start and Escape, which open and close
    /// the pause and the menus, so no binding a player remaps can have them.
    #[must_use]
    pub const fn cancel(self) -> Source {
        match self {
            Self::Gamepad => Source::Pad(PadButton::Start),
            Self::Keyboard => Source::Key(Key::Escape),
        }
    }
}

/// One of the player's bindings, laid over the game's document: the binding by its name, and the source
/// it reads in place of the document's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Remapped {
    pub binding: String,
    pub source: Source,
}

/// `base` with the player's bindings over it: each of `remapped` reading its source, and each action in
/// `toggled` toggling ([`Trigger::Toggle`]) where it read a level (`Down` or `DownFromPress`). A remap the
/// document can't take is dropped with a log line, never an error, since a player's file is input from
/// outside: a binding the document has no name for, or a source another device's or one a binding can't
/// read (a stick). So is a toggle of an action that isn't a button.
#[must_use]
pub fn layered(base: &InputBindings, remapped: &[Remapped], toggled: &[&str]) -> InputBindings {
    let mut bindings = base.clone();
    for remap in remapped {
        let found = bindings
            .contexts
            .iter_mut()
            .flat_map(|context| &mut context.bindings)
            .find(|binding| binding.name.as_deref() == Some(remap.binding.as_str()));
        let device = RemapDevice::of_binding(&remap.binding);
        match (found, device) {
            (Some(binding), Some(device)) if device.accepts(&remap.source) => {
                binding.source = remap.source.clone()
            }
            (found, _) => {
                let why = if found.is_none() {
                    "the bindings have no binding by that name"
                } else {
                    "it can't read that"
                };
                tracing::warn!(
                    target: crate::LOG_TARGET,
                    binding = %remap.binding,
                    source = ?remap.source,
                    why,
                    "a saved binding is dropped"
                );
            }
        }
    }
    for action in toggled {
        let button = base
            .actions
            .iter()
            .any(|desc| desc.name == *action && desc.kind == sapphire_input::ActionKind::Button);
        if !button {
            tracing::warn!(target: crate::LOG_TARGET, action, "only a button the bindings have can toggle");
            continue;
        }
        for binding in bindings.contexts.iter_mut().flat_map(|context| &mut context.bindings) {
            if binding.action == *action && matches!(binding.trigger, Trigger::Down | Trigger::DownFromPress)
            {
                binding.trigger = Trigger::Toggle;
            }
        }
    }
    bindings
}

/// What `name` reads in `bindings`, if they have it.
#[must_use]
pub fn source_of<'a>(bindings: &'a InputBindings, name: &str) -> Option<&'a Source> {
    bindings
        .contexts
        .iter()
        .flat_map(|context| &context.bindings)
        .find(|binding| binding.name.as_deref() == Some(name))
        .map(|binding| &binding.source)
}

/// Makes `binding` read `source` among the player's `remapped`, over `base`: a source that's the
/// document's own takes the remap out, so the file keeps only what differs.
pub fn set(remapped: &mut Vec<Remapped>, base: &InputBindings, binding: &str, source: Source) {
    remapped.retain(|remap| remap.binding != binding);
    if source_of(base, binding) != Some(&source) {
        remapped.push(Remapped { binding: binding.to_owned(), source });
    }
}

/// Takes out every remap of `device`'s bindings: back to the document.
pub fn reset(remapped: &mut Vec<Remapped>, device: RemapDevice) {
    remapped.retain(|remap| RemapDevice::of_binding(&remap.binding) != Some(device));
}

/// Who else reads a source a binding is to read, in the binding's own context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Taken<'a> {
    /// Another binding of the same device, which a player remaps too: a swap gives it what the first read.
    Swappable { binding: &'a str, action: &'a str },
    /// A binding no player remaps (the pause's, the camera's keys): the source is kept for its action.
    Kept { action: &'a str },
}

/// Whether `source` is another binding's in `binding`'s context of `bindings`. A binding a player can't
/// remap wins over one they can, since a swap would leave it reading the source too.
#[must_use]
pub fn taken<'a>(bindings: &'a InputBindings, binding: &str, source: &Source) -> Option<Taken<'a>> {
    let context = bindings
        .contexts
        .iter()
        .find(|context| context.bindings.iter().any(|desc| desc.name.as_deref() == Some(binding)))?;
    let device = RemapDevice::of_binding(binding);
    let mut found = None;
    for other in &context.bindings {
        if other.source != *source || other.name.as_deref() == Some(binding) {
            continue;
        }
        match other.name.as_deref() {
            Some(name) if device.is_some() && RemapDevice::of_binding(name) == device => {
                found = found.or(Some(Taken::Swappable { binding: name, action: &other.action }));
            }
            _ => return Some(Taken::Kept { action: &other.action }),
        }
    }
    found
}

/// What a prompt shows for `source`: a key by its name on a US layout, which keyboards print in Latin
/// letters in every language the game speaks, or a pad's button as `family` labels it. Every character is
/// in the interface's fonts' set beyond what documents bind: ASCII, Latin-1, arrows and geometric shapes.
#[must_use]
pub fn glyph(source: &Source, family: GamepadFamily) -> Cow<'static, str> {
    match source {
        Source::Key(key) => key_glyph(*key),
        Source::Mouse(button) => Cow::Borrowed(match button {
            MouseButton::Left => "Mouse L",
            MouseButton::Middle => "Mouse M",
            MouseButton::Right => "Mouse R",
            MouseButton::Back => "Mouse 4",
            MouseButton::Forward => "Mouse 5",
            MouseButton::Other(n) => return Cow::Owned(format!("Mouse {n}")),
        }),
        Source::MouseMotion => Cow::Borrowed("Mouse"),
        Source::MouseWheel | Source::MouseWheelHorizontal => Cow::Borrowed("Wheel"),
        Source::Pad(button) => Cow::Borrowed(pad_glyph(*button, family)),
        Source::PadAxis(axis) => Cow::Borrowed(axis_glyph(*axis, family)),
        Source::PadStick(Stick::Left) => Cow::Borrowed(stick_glyph(true, family)),
        Source::PadStick(Stick::Right) => Cow::Borrowed(stick_glyph(false, family)),
        Source::Touch(_) | Source::Virtual(_) => Cow::Borrowed("Touch"),
    }
}

fn key_glyph(key: Key) -> Cow<'static, str> {
    let debug = format!("{key:?}");
    let named = match key {
        Key::Digit0 => "0",
        Key::Digit1 => "1",
        Key::Digit2 => "2",
        Key::Digit3 => "3",
        Key::Digit4 => "4",
        Key::Digit5 => "5",
        Key::Digit6 => "6",
        Key::Digit7 => "7",
        Key::Digit8 => "8",
        Key::Digit9 => "9",
        Key::Return => "Enter",
        Key::Escape => "Esc",
        Key::Backspace => "Backspace",
        Key::Tab => "Tab",
        Key::Space => "Space",
        Key::Minus => "-",
        Key::Equals => "=",
        Key::LeftBracket => "[",
        Key::RightBracket => "]",
        Key::Backslash => "\\",
        Key::Semicolon => ";",
        Key::Apostrophe => "'",
        Key::Grave => "`",
        Key::Comma => ",",
        Key::Period => ".",
        Key::Slash => "/",
        Key::CapsLock => "Caps Lock",
        Key::PrintScreen => "PrtSc",
        Key::ScrollLock => "ScrLk",
        Key::Pause => "Pause",
        Key::Insert => "Insert",
        Key::Home => "Home",
        Key::PageUp => "PgUp",
        Key::Delete => "Delete",
        Key::End => "End",
        Key::PageDown => "PgDn",
        Key::Right => "\u{2192}",
        Key::Left => "\u{2190}",
        Key::Down => "\u{2193}",
        Key::Up => "\u{2191}",
        Key::LeftCtrl => "Ctrl",
        Key::LeftShift => "Shift",
        Key::LeftAlt => "Alt",
        Key::LeftGui => "Meta",
        Key::RightCtrl => "R Ctrl",
        Key::RightShift => "R Shift",
        Key::RightAlt => "R Alt",
        Key::RightGui => "R Meta",
        Key::Hid(usage) => return Cow::Owned(format!("Key {usage}")),
        // A to Z and F1 to F12 are their own names.
        _ => return Cow::Owned(debug),
    };
    Cow::Borrowed(named)
}

fn pad_glyph(button: PadButton, family: GamepadFamily) -> &'static str {
    use GamepadFamily::{Nintendo, PlayStation};
    match (button, family) {
        (PadButton::South, PlayStation) => "\u{d7}",
        (PadButton::East, PlayStation) => "\u{25cb}",
        (PadButton::West, PlayStation) => "\u{25a1}",
        (PadButton::North, PlayStation) => "\u{25b3}",
        // Nintendo's labels sit a quarter turn round from Xbox's.
        (PadButton::South, Nintendo) => "B",
        (PadButton::East, Nintendo) => "A",
        (PadButton::West, Nintendo) => "Y",
        (PadButton::North, Nintendo) => "X",
        (PadButton::South, _) => "A",
        (PadButton::East, _) => "B",
        (PadButton::West, _) => "X",
        (PadButton::North, _) => "Y",
        (PadButton::Back, PlayStation) => "Create",
        (PadButton::Back, Nintendo) => "-",
        (PadButton::Back, _) => "View",
        (PadButton::Start, PlayStation) => "Options",
        (PadButton::Start, Nintendo) => "+",
        (PadButton::Start, _) => "Menu",
        (PadButton::Guide, PlayStation) => "PS",
        (PadButton::Guide, _) => "Home",
        (PadButton::LeftStick, PlayStation) => "L3",
        (PadButton::RightStick, PlayStation) => "R3",
        (PadButton::LeftStick, _) => "LS",
        (PadButton::RightStick, _) => "RS",
        (PadButton::LeftShoulder, PlayStation) => "L1",
        (PadButton::RightShoulder, PlayStation) => "R1",
        (PadButton::LeftShoulder, Nintendo) => "L",
        (PadButton::RightShoulder, Nintendo) => "R",
        (PadButton::LeftShoulder, _) => "LB",
        (PadButton::RightShoulder, _) => "RB",
        (PadButton::DpadUp, _) => "D-pad \u{2191}",
        (PadButton::DpadDown, _) => "D-pad \u{2193}",
        (PadButton::DpadLeft, _) => "D-pad \u{2190}",
        (PadButton::DpadRight, _) => "D-pad \u{2192}",
        (PadButton::Misc1, PlayStation) => "Mute",
        (PadButton::Misc1, Nintendo) => "Capture",
        (PadButton::Misc1, _) => "Share",
        (PadButton::RightPaddle1, _) => "P1",
        (PadButton::LeftPaddle1, _) => "P3",
        (PadButton::RightPaddle2, _) => "P2",
        (PadButton::LeftPaddle2, _) => "P4",
        (PadButton::Touchpad, _) => "Touchpad",
        (PadButton::Misc2, _) => "Misc 2",
        (PadButton::Misc3, _) => "Misc 3",
        (PadButton::Misc4, _) => "Misc 4",
        (PadButton::Misc5, _) => "Misc 5",
        (PadButton::Misc6, _) => "Misc 6",
    }
}

fn axis_glyph(axis: PadAxis, family: GamepadFamily) -> &'static str {
    use GamepadFamily::{Nintendo, PlayStation};
    match (axis, family) {
        (PadAxis::LeftTrigger, PlayStation) => "L2",
        (PadAxis::RightTrigger, PlayStation) => "R2",
        (PadAxis::LeftTrigger, Nintendo) => "ZL",
        (PadAxis::RightTrigger, Nintendo) => "ZR",
        (PadAxis::LeftTrigger, _) => "LT",
        (PadAxis::RightTrigger, _) => "RT",
        (PadAxis::LeftX | PadAxis::LeftY, _) => stick_glyph(true, family),
        (PadAxis::RightX | PadAxis::RightY, _) => stick_glyph(false, family),
    }
}

fn stick_glyph(left: bool, family: GamepadFamily) -> &'static str {
    match (left, family) {
        (true, GamepadFamily::PlayStation) => "L",
        (false, GamepadFamily::PlayStation) => "R",
        (true, _) => "LS",
        (false, _) => "RS",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::default_bindings;

    fn remap(binding: &str, source: Source) -> Remapped {
        Remapped { binding: binding.to_owned(), source }
    }

    #[test]
    fn a_remap_lays_over_the_document_and_a_broken_one_drops_alone() {
        let base = default_bindings().expect("the default bindings");
        let remapped = [
            remap("light.gamepad", Source::Pad(PadButton::DpadUp)),
            remap("nobody.gamepad", Source::Pad(PadButton::North)),
            remap("heavy.gamepad", Source::Key(Key::K)),
            remap("dodge.gamepad", Source::PadStick(Stick::Left)),
            remap("jump.keyboard", Source::Key(Key::C)),
        ];
        let layered = layered(&base, &remapped, &[]);
        assert_eq!(source_of(&layered, "light.gamepad"), Some(&Source::Pad(PadButton::DpadUp)));
        assert_eq!(source_of(&layered, "jump.keyboard"), Some(&Source::Key(Key::C)));
        assert_eq!(
            source_of(&layered, "heavy.gamepad"),
            source_of(&base, "heavy.gamepad"),
            "a key on the pad"
        );
        assert_eq!(source_of(&layered, "dodge.gamepad"), source_of(&base, "dodge.gamepad"), "a stick");
        sapphire_input::InputMap::new(layered).expect("it compiles");
    }

    #[test]
    fn a_toggle_takes_the_levels_of_its_action_and_nothing_else() {
        let base = default_bindings().expect("the default bindings");
        let layered = layered(&base, &[], &["dodge", "move", "nothing"]);
        for context in &layered.contexts {
            for binding in &context.bindings {
                let was = base
                    .contexts
                    .iter()
                    .flat_map(|context| &context.bindings)
                    .find(|other| other.action == binding.action && other.source == binding.source)
                    .expect("the same binding");
                let expected = if binding.action == "dodge" { Trigger::Toggle } else { was.trigger };
                assert_eq!(binding.trigger, expected, "{} on {:?}", binding.action, binding.source);
            }
        }
        sapphire_input::InputMap::new(layered).expect("it compiles");
    }

    #[test]
    fn a_remap_back_to_the_document_leaves_the_file() {
        let base = default_bindings().expect("the default bindings");
        let mut remapped = Vec::new();
        set(&mut remapped, &base, "light.gamepad", Source::Pad(PadButton::DpadUp));
        set(&mut remapped, &base, "jump.keyboard", Source::Key(Key::C));
        assert_eq!(remapped.len(), 2);
        set(&mut remapped, &base, "light.gamepad", Source::Pad(PadButton::West));
        assert_eq!(remapped, [remap("jump.keyboard", Source::Key(Key::C))], "West is the document's");
        set(&mut remapped, &base, "light.gamepad", Source::Pad(PadButton::DpadUp));
        reset(&mut remapped, RemapDevice::Gamepad);
        assert_eq!(remapped, [remap("jump.keyboard", Source::Key(Key::C))], "the keyboard's stays");
        reset(&mut remapped, RemapDevice::Keyboard);
        assert!(remapped.is_empty());
    }

    #[test]
    fn a_press_is_free_another_remappable_bindings_or_kept() {
        let base = default_bindings().expect("the default bindings");
        let east = Source::Pad(PadButton::East);
        assert_eq!(
            taken(&base, "light.gamepad", &east),
            Some(Taken::Swappable { binding: "heavy.gamepad", action: "heavy" })
        );
        assert_eq!(taken(&base, "light.gamepad", &Source::Pad(PadButton::West)), None, "its own");
        assert_eq!(taken(&base, "light.gamepad", &Source::Pad(PadButton::DpadUp)), None);
        assert_eq!(
            taken(&base, "light.gamepad", &Source::Pad(PadButton::Back)),
            Some(Taken::Kept { action: "pause" })
        );
        assert_eq!(
            taken(&base, "jump.keyboard", &Source::Key(Key::Up)),
            Some(Taken::Kept { action: "look" })
        );
        assert_eq!(
            taken(&base, "jump.keyboard", &Source::Key(Key::P)),
            Some(Taken::Kept { action: "pause" })
        );
        assert_eq!(
            taken(&base, "jump.keyboard", &Source::Key(Key::W)),
            Some(Taken::Swappable { binding: "move_forward.keyboard", action: "move" })
        );
        // Start and Escape end a capture, so no remap can reach the pause's own.
        for device in RemapDevice::ALL {
            let pause = format!("pause{}", device.suffix());
            assert_eq!(source_of(&base, &pause), Some(&device.cancel()));
        }
    }

    #[test]
    fn a_glyph_follows_the_pads_labels() {
        let west = Source::Pad(PadButton::West);
        assert_eq!(glyph(&west, GamepadFamily::Xbox), "X");
        assert_eq!(glyph(&west, GamepadFamily::PlayStation), "\u{25a1}");
        assert_eq!(glyph(&west, GamepadFamily::Nintendo), "Y");
        assert_eq!(glyph(&Source::PadAxis(PadAxis::RightTrigger), GamepadFamily::Generic), "RT");
        assert_eq!(glyph(&Source::Key(Key::J), GamepadFamily::Xbox), "J");
        assert_eq!(glyph(&Source::Key(Key::F11), GamepadFamily::Xbox), "F11");
        assert_eq!(glyph(&Source::Key(Key::LeftShift), GamepadFamily::Xbox), "Shift");
        assert_eq!(glyph(&Source::Key(Key::Hid(282)), GamepadFamily::Xbox), "Key 282");
    }
}
