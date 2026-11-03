//! Touch's road to a tick: the zones say what the fingers did, and `read_touch` hands that to
//! the bindings. Every rule a pad has holds here too: a press is one edge however frames and steps line
//! up, a stick is a level turned by the camera, and what ends a press is `sapphire_input`'s.

#[path = "../../../crates/sapphire_controls/tests/support/mod.rs"]
mod support;

use std::f32::consts::FRAC_PI_2;

use game_ui::touch::TouchControls;
use sapphire_controls::{CombatControls, TOUCH_BUTTONS, default_bindings};
use sapphire_gameplay::{Buttons, Stick};
use sapphire_input::{InputMap, RawInput, Source, Trigger};
use sapphire_math::{Vec2, Vec3, det};
use sapphire_platform::{Event, FingerId, LifecycleEvent, TouchDeviceId, TouchPhase, WindowEvent, WindowId};
use sapphire_ui::{Button, HudLayout, Insets, UiMetrics};
use support::{BACK, D, FRAME_MS, L, N, Rig, key, ms};

const SKILL: Buttons = Buttons::SKILL;
const THUMB: u64 = 1;
const OTHER_THUMB: u64 = 2;

fn finger(id: u64) -> FingerId {
    FingerId::new(TouchDeviceId::from_raw(1), id)
}

/// The zones of a phone in landscape over combat's controls, a frame at a time as the app runs them.
struct Phone {
    rig: Rig,
    zones: TouchControls,
    layout: HudLayout,
}

impl Phone {
    fn new() -> Self {
        Self::on(Rig::new())
    }

    fn on(rig: Rig) -> Self {
        let layout = HudLayout::new(UiMetrics::new(Vec2::new(900.0, 400.0), Insets::default()));
        Self { rig, zones: TouchControls::new(layout), layout }
    }

    /// A finger's event `offset` milliseconds into the frame under way.
    fn touch(&mut self, offset: u64, id: u64, phase: TouchPhase, at: Vec2) {
        self.zones.touch(ms(self.rig.now + offset), finger(id), phase, at);
    }

    fn press(&mut self, offset: u64, id: u64, button: Button) {
        let at = self.layout.button(button).center;
        self.touch(offset, id, TouchPhase::Down, at);
    }

    fn lift(&mut self, offset: u64, id: u64, button: Button) {
        let at = self.layout.button(button).center;
        self.touch(offset, id, TouchPhase::Up, at);
    }

    /// A thumb on the stick, pushed by `by` of its radius with +y forward.
    fn push_stick(&mut self, offset: u64, id: u64, by: Vec2) {
        let (rest, radius) = (self.layout.stick.rest, self.layout.stick.radius);
        self.touch(offset, id, TouchPhase::Down, rest);
        self.touch(offset + 1, id, TouchPhase::Moved, rest + Vec2::new(by.x, -by.y) * radius);
    }

    /// One frame: the zones publish, the controls read them, and the frame's input is judged.
    fn frame(&mut self) {
        let now = ms(self.rig.now + FRAME_MS);
        let touch = self.zones.update(now);
        self.rig.controls.read_touch(touch, now);
        self.rig.finish_frame();
    }

    /// A platform event, to the zones and then to the input, as the app routes it.
    fn platform(&mut self, event: &Event) {
        if !self.zones.handle_event(event) {
            self.rig.controls.handle_event(event);
        }
    }

    fn presses(&self) -> u64 {
        self.rig.controls.stats().presses
    }
}

/// The default bindings with the on-screen controls named here on these triggers: their bindings have
/// no names of their own, as nothing rebinds them yet.
fn on_screen_rebound(name: &str, trigger: Trigger) -> Rig {
    let mut document = default_bindings().expect("the default bindings");
    let wanted = Source::Virtual(name.into());
    let binding = document
        .contexts
        .iter_mut()
        .flat_map(|context| &mut context.bindings)
        .find(|binding| binding.source == wanted)
        .expect("an on-screen binding");
    binding.trigger = trigger;
    Rig::on(CombatControls::new(InputMap::new(document).expect("compiles")).expect("combat's actions"))
}

#[test]
fn every_button_presses_a_verb_of_the_bindings() {
    let map = InputMap::new(default_bindings().expect("the default bindings")).expect("compiles");
    for (button, name) in TOUCH_BUTTONS {
        assert!(map.virtual_control(name).is_some(), "{button:?}: no on-screen `{name}`");
    }
    for button in Button::ALL {
        assert!(TOUCH_BUTTONS.iter().any(|(mapped, _)| *mapped == button), "{button:?} presses nothing");
    }
}

#[test]
fn a_button_held_is_one_press_of_its_verb() {
    for (button, verb) in [(Button::Attack, L), (Button::Dodge, D), (Button::Skill, SKILL)] {
        let mut phone = Phone::new();
        phone.press(1, THUMB, button);
        phone.frame();
        assert_eq!(phone.rig.steps(2), [verb, verb], "{button:?}");
        // On through its hold time, which is the button's own business.
        for _ in 0..30 {
            phone.frame();
            assert_eq!(phone.rig.steps(1), [verb], "{button:?}");
        }
        assert_eq!(phone.presses(), 1, "{button:?}");
        phone.lift(1, THUMB, button);
        phone.frame();
        assert_eq!(phone.rig.steps(1), [N], "{button:?}");
        phone.press(1, THUMB, button);
        phone.frame();
        assert_eq!(phone.rig.steps(1), [verb], "{button:?}");
        assert_eq!(phone.presses(), 2, "{button:?}");
    }
}

/// At 30 fps a quick tap comes and goes inside a frame, and a frame may run no step at all.
#[test]
fn a_tap_inside_one_frame_is_one_press_however_steps_fall() {
    let mut phone = Phone::new();
    phone.press(1, THUMB, Button::Attack);
    phone.lift(5, THUMB, Button::Attack);
    phone.frame();
    assert_eq!(phone.rig.steps(3), [L, N, N]);

    // Two taps in one frame are two presses, an edge a step.
    phone.press(1, THUMB, Button::Dodge);
    phone.lift(2, THUMB, Button::Dodge);
    phone.press(3, THUMB, Button::Dodge);
    phone.lift(4, THUMB, Button::Dodge);
    phone.frame();
    assert_eq!(phone.rig.steps(4), [D, N, D, N]);

    // A frame with no step keeps it for the next.
    phone.press(1, THUMB, Button::Skill);
    phone.lift(2, THUMB, Button::Skill);
    phone.frame();
    phone.frame();
    assert_eq!(phone.rig.steps(2), [SKILL, N]);
}

#[test]
fn the_stick_reads_as_a_pads_does() {
    let mut phone = Phone::new();
    phone.push_stick(1, THUMB, Vec2::new(1.0, 0.0));
    phone.frame();
    assert_eq!(phone.rig.next().stick, Stick::new(127, 0), "full to the right");
    phone.frame();
    assert_eq!(phone.rig.next().stick, Stick::new(127, 0), "a level: it stays while the thumb does");

    let rest = phone.layout.stick.rest;
    phone.touch(1, THUMB, TouchPhase::Up, rest);
    phone.frame();
    assert_eq!(phone.rig.next().stick, Stick::CENTER, "exactly centred once it lifts");

    // Forward is forward, up the screen.
    phone.push_stick(1, THUMB, Vec2::new(0.0, 1.0));
    phone.frame();
    assert_eq!(phone.rig.next().stick, Stick::new(0, 127));
    phone.touch(1, THUMB, TouchPhase::Up, rest);
    phone.frame();

    // Inside the dead zone it reads nothing, exactly.
    phone.push_stick(1, THUMB, Vec2::new(0.05, 0.0));
    phone.frame();
    assert_eq!(phone.rig.next().stick, Stick::CENTER);
}

/// What a press means under a turned camera: forward walks away from the view and right to its right,
/// at any yaw, read off the convention as `world_stick.rs` does.
#[test]
fn the_stick_goes_where_the_view_goes() {
    for yaw in [0.0, 0.35, 1.0, FRAC_PI_2, 2.5, -2.0, 40.0] {
        let forward = det::direction_from_yaw(yaw);
        let right = forward.cross(Vec3::Y);
        for (push, want) in [
            (Vec2::new(0.0, 1.0), forward),
            (Vec2::new(1.0, 0.0), right),
            (Vec2::new(0.0, -1.0), -forward),
            (Vec2::new(-1.0, 0.0), -right),
        ] {
            let mut phone = Phone::new();
            phone.push_stick(1, THUMB, push);
            phone.frame();
            let stick = phone.rig.controls.next_actions(yaw).stick.to_vec2();
            let got = Vec3::new(stick.x, 0.0, stick.y);
            assert!(got.distance(want) < 0.02, "pushed {push} at yaw {yaw}: {got}, wanted {want}");
        }
    }
}

#[test]
fn a_thumb_on_the_stick_and_a_verb_go_together() {
    let mut phone = Phone::new();
    phone.push_stick(1, THUMB, Vec2::new(0.0, 1.0));
    phone.press(4, OTHER_THUMB, Button::Attack);
    phone.frame();
    let step = phone.rig.next();
    assert_eq!((step.buttons, step.stick), (L, Stick::new(0, 127)));

    // The stick keeps going under another press.
    phone.lift(1, OTHER_THUMB, Button::Attack);
    phone.press(2, OTHER_THUMB, Button::Dodge);
    phone.frame();
    let step = phone.rig.next();
    assert_eq!((step.buttons, step.stick), (D, Stick::new(0, 127)));
}

#[test]
fn a_finger_the_os_takes_back_ends_only_what_it_holds() {
    let mut phone = Phone::new();
    phone.push_stick(1, THUMB, Vec2::new(0.0, 1.0));
    phone.press(4, OTHER_THUMB, Button::Attack);
    phone.frame();
    assert_eq!(phone.rig.next().buttons, L);

    let at = phone.layout.attack.center;
    phone.touch(1, OTHER_THUMB, TouchPhase::Canceled, at);
    phone.frame();
    let step = phone.rig.next();
    assert_eq!((step.buttons, step.stick), (N, Stick::new(0, 127)), "the attack's gone, the stick isn't");

    phone.press(1, OTHER_THUMB, Button::Dodge);
    phone.frame();
    let rest = phone.layout.stick.rest;
    phone.touch(1, THUMB, TouchPhase::Canceled, rest);
    phone.frame();
    let _ = phone.rig.next();
    let step = phone.rig.next();
    assert_eq!((step.buttons, step.stick), (D, Stick::CENTER), "the stick's gone, the dodge isn't");
}

/// The table in the crate docs: a press already made arrives, and a canceled finger fires nothing a
/// release would, where a lifted one does.
#[test]
fn a_cancel_fires_nothing_a_release_would() {
    const TAP: Trigger = Trigger::Tap { seconds: 0.2 };

    let mut phone = Phone::on(on_screen_rebound("light", TAP));
    phone.press(1, THUMB, Button::Attack);
    phone.frame();
    assert_eq!(phone.rig.steps(1), [N], "held, no tap yet");
    phone.lift(1, THUMB, Button::Attack);
    phone.frame();
    assert_eq!(phone.rig.steps(2), [L, N], "lifted, it's a tap");

    let mut phone = Phone::on(on_screen_rebound("light", TAP));
    phone.press(1, THUMB, Button::Attack);
    phone.frame();
    assert_eq!(phone.rig.steps(1), [N]);
    let at = phone.layout.attack.center;
    phone.touch(1, THUMB, TouchPhase::Canceled, at);
    phone.frame();
    assert_eq!(phone.rig.steps(2), [N, N], "taken back, it's nothing");

    // A press made before the cancel arrives, in the same frame too.
    let mut phone = Phone::new();
    phone.press(1, THUMB, Button::Attack);
    phone.touch(2, THUMB, TouchPhase::Canceled, phone.layout.attack.center);
    phone.frame();
    assert_eq!(phone.rig.steps(2), [L, N]);
}

#[test]
fn losing_focus_or_the_foreground_lets_go_of_everything() {
    let window = WindowId::from_raw(1);
    let losses = [
        Event::Window { timestamp: ms(40), window, event: WindowEvent::FocusLost },
        Event::Lifecycle { timestamp: ms(40), event: LifecycleEvent::WillEnterBackground },
    ];
    for loss in &losses {
        let mut phone = Phone::new();
        phone.push_stick(1, THUMB, Vec2::new(0.0, 1.0));
        phone.press(4, OTHER_THUMB, Button::Attack);
        phone.frame();
        assert_eq!(phone.rig.next().buttons, L);

        phone.platform(loss);
        phone.frame();
        let step = phone.rig.next();
        assert_eq!((step.buttons, step.stick), (N, Stick::CENTER), "{loss:?}");
        assert_eq!(phone.zones.fingers(), 0, "{loss:?}");
        assert_eq!(phone.presses(), 1, "{loss:?}: letting go presses nothing");

        // Their real releases come late, or never, and find nobody.
        let event = sapphire_platform::TouchEvent {
            timestamp: ms(phone.rig.now + 1),
            window,
            finger: finger(OTHER_THUMB),
            phase: TouchPhase::Up,
            x: 0.5,
            y: 0.5,
            pressure: 1.0,
        };
        phone.platform(&Event::Touch(event));
        phone.frame();
        assert_eq!(phone.rig.next().buttons, N, "{loss:?}");

        // And back: a new thumb presses as if nothing had happened.
        phone.press(1, OTHER_THUMB, Button::Attack);
        phone.frame();
        assert_eq!(phone.rig.next().buttons, L, "{loss:?}");
        assert_eq!(phone.presses(), 2, "{loss:?}");
    }
}

/// The pause takes combat's bindings away, so nothing a finger does reaches a tick, and the thumb still
/// down when it's over reads released until it presses again.
#[test]
fn nothing_touch_does_passes_through_the_pause() {
    let mut phone = Phone::new();
    phone.press(1, THUMB, Button::Attack);
    phone.frame();
    assert_eq!(phone.rig.next().buttons, L);
    phone.rig.controls.input_mut().push(ms(phone.rig.now + 1), key(BACK, true));
    phone.rig.controls.input_mut().push(ms(phone.rig.now + 2), key(BACK, false));
    phone.frame();
    assert!(phone.rig.controls.is_paused());

    // Pressed and let go during the pause: it never reaches the steps.
    phone.press(1, OTHER_THUMB, Button::Dodge);
    phone.lift(5, OTHER_THUMB, Button::Dodge);
    phone.frame();
    assert_eq!(phone.presses(), 1);
    assert_eq!(phone.rig.controls.look(), Vec2::ZERO);

    phone.rig.controls.set_paused(false);
    phone.frame();
    phone.frame();
    assert!(!phone.rig.controls.is_paused());
    assert_eq!(phone.rig.steps(2), [N, N], "the button that was down isn't pressed again");
    assert_eq!(phone.presses(), 1);
    phone.lift(1, THUMB, Button::Attack);
    phone.frame();
    phone.press(1, THUMB, Button::Attack);
    phone.frame();
    assert_eq!(phone.rig.next().buttons, L);
}

/// A thumb's press reaches the bindings after the frame's pump, whenever it happened. It keeps to its
/// own side of a Back in that frame by its time, as a pad's press does ("presses keep to their side of
/// the pause"), not by which road got there first.
#[test]
fn a_thumb_and_back_in_one_frame_keep_to_their_sides_of_the_pause() {
    // The thumb first: it arrives, on the first step after the pause, as if no time had passed.
    let mut phone = Phone::new();
    phone.press(1, THUMB, Button::Attack);
    phone.rig.controls.input_mut().push(ms(phone.rig.now + 3), key(BACK, true));
    phone.rig.controls.input_mut().push(ms(phone.rig.now + 4), key(BACK, false));
    phone.frame();
    assert!(phone.rig.controls.is_paused());
    assert_eq!(phone.presses(), 1, "pressed before the pause");
    phone.rig.controls.set_paused(false);
    phone.frame();
    assert!(!phone.rig.controls.is_paused());
    assert_eq!(phone.rig.next().buttons, L);

    // Back first: the thumb's is the pause's, however the frame's input is put together.
    let mut phone = Phone::new();
    phone.rig.controls.input_mut().push(ms(phone.rig.now + 1), key(BACK, true));
    phone.rig.controls.input_mut().push(ms(phone.rig.now + 2), key(BACK, false));
    phone.press(3, THUMB, Button::Attack);
    phone.frame();
    assert!(phone.rig.controls.is_paused());
    assert_eq!(phone.presses(), 0, "pressed after the pause");
    phone.rig.controls.set_paused(false);
    phone.frame();
    assert_eq!(phone.rig.steps(2), [N, N], "and held on, it isn't pressed now");
}

/// The camera's drag is the bindings' too, at the rate of `Touch(Drag)`: 0.006 rad a point, dragging
/// right turning toward negative yaw. It lasts the update it arrived in.
#[test]
fn a_drag_anywhere_else_turns_the_camera_at_the_bindings_rate() {
    let mut phone = Phone::new();
    let middle = Vec2::new(450.0, 150.0);
    phone.touch(1, THUMB, TouchPhase::Down, middle);
    phone.touch(2, THUMB, TouchPhase::Moved, middle + Vec2::new(20.0, -10.0));
    phone.frame();
    let look = phone.rig.controls.look();
    assert!((look.x + 0.12).abs() < 1e-5 && (look.y + 0.06).abs() < 1e-5, "{look}");
    assert_eq!(phone.rig.next().stick, Stick::CENTER, "it's no stick");

    phone.frame();
    assert_eq!(phone.rig.controls.look(), Vec2::ZERO, "a finger held still turns nothing");

    // Against what the gesture's binding gives for the same drag.
    let mut raw = Rig::new();
    let drag = |phase, position| RawInput::Touch { finger: 0, phase, position };
    let moved = middle + Vec2::new(20.0, -10.0);
    raw.controls.input_mut().push(ms(raw.now + 1), drag(TouchPhase::Down, middle));
    raw.controls.input_mut().push(ms(raw.now + 2), drag(TouchPhase::Moved, moved));
    raw.finish_frame();
    assert_eq!(raw.controls.look(), look);
}
