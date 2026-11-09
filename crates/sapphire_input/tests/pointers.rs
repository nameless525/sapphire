//! What pointers did, in time order and in points, for a screen's hit test.

use sapphire_input::{
    InputBindings, InputMap, InputSystem, MAX_FINGERS, MouseButton, Pointer, PointerEvent, PointerPhase,
    RawInput, Timestamp,
};
use sapphire_math::Vec2;
use sapphire_platform::{
    Event, MouseButton as PlatformButton, MouseButtonEvent, MouseMotionEvent, TouchPhase, WindowEvent,
    WindowId,
};

fn ms(ms: u64) -> Timestamp {
    Timestamp::from_nanos(ms * 1_000_000)
}

fn system() -> InputSystem {
    InputSystem::new(InputMap::new(InputBindings::default()).unwrap())
}

fn seen(system: &InputSystem) -> Vec<(Pointer, PointerPhase, Vec2)> {
    system
        .pointers()
        .iter()
        .map(|&PointerEvent { pointer, phase, position, .. }| (pointer, phase, position))
        .collect()
}

fn motion(at: u64, x: f32, y: f32) -> Event {
    Event::MouseMotion(MouseMotionEvent {
        timestamp: ms(at),
        window: WindowId::from_raw(1),
        x,
        y,
        dx: 1.0,
        dy: 1.0,
    })
}

fn button(at: u64, down: bool, x: f32, y: f32) -> Event {
    Event::MouseButton(MouseButtonEvent {
        timestamp: ms(at),
        window: WindowId::from_raw(1),
        button: PlatformButton::Left,
        down,
        clicks: 1,
        x,
        y,
    })
}

#[test]
fn the_mouse_says_where_it_is_in_points_and_only_its_left_button_presses() {
    let mut system = system();
    system.set_points_per_coordinate(2.0);
    system.handle_event(&motion(1, 10.0, 20.0));
    system.handle_event(&button(2, true, 15.0, 20.0));
    system.handle_event(&button(3, false, 15.0, 20.0));
    system.update(ms(4));
    let (at, moved) = (Vec2::new(20.0, 40.0), Vec2::new(30.0, 40.0));
    assert_eq!(
        seen(&system),
        [
            (Pointer::Mouse, PointerPhase::Moved, at),
            (Pointer::Mouse, PointerPhase::Moved, moved),
            (Pointer::Mouse, PointerPhase::Down, moved),
            (Pointer::Mouse, PointerPhase::Up, moved),
        ]
    );
    // An update's list is that update's.
    system.update(ms(5));
    assert_eq!(seen(&system), []);

    system.push(ms(6), RawInput::MouseButton { button: MouseButton::Right, down: true });
    system.push(ms(6), RawInput::MouseAt { position: moved });
    system.update(ms(7));
    assert_eq!(seen(&system), [], "the right button, and a mouse that hasn't moved");
}

#[test]
fn fingers_are_told_apart() {
    let mut system = system();
    let (a, b) = (Vec2::new(1.0, 2.0), Vec2::new(3.0, 4.0));
    system.push(ms(1), RawInput::Touch { finger: 0, phase: TouchPhase::Down, position: a });
    system.push(ms(2), RawInput::Touch { finger: 1, phase: TouchPhase::Down, position: b });
    system.push(ms(3), RawInput::Touch { finger: 0, phase: TouchPhase::Moved, position: b });
    system.push(ms(4), RawInput::Touch { finger: 1, phase: TouchPhase::Canceled, position: b });
    system.push(ms(5), RawInput::Touch { finger: 0, phase: TouchPhase::Up, position: b });
    system.update(ms(6));
    assert_eq!(
        seen(&system),
        [
            (Pointer::Finger(0), PointerPhase::Down, a),
            (Pointer::Finger(1), PointerPhase::Down, b),
            (Pointer::Finger(0), PointerPhase::Moved, b),
            (Pointer::Finger(1), PointerPhase::Canceled, b),
            (Pointer::Finger(0), PointerPhase::Up, b),
        ]
    );
}

/// `SDL_SetKeyboardFocus(NULL)` and Wayland's pointer leave post ordinary releases right before the
/// loss. A press these take back never ended, so it's canceled, and nothing it would have done happens.
#[test]
fn a_release_that_comes_with_a_loss_of_focus_cancels() {
    let mut system = system();
    system.handle_event(&button(1, true, 10.0, 10.0));
    system.push(ms(1), RawInput::Touch { finger: 2, phase: TouchPhase::Down, position: Vec2::ONE });
    system.update(ms(2));
    assert_eq!(system.pointers().len(), 3);

    system.handle_event(&button(3, false, 10.0, 10.0));
    system.handle_event(&Event::Window {
        timestamp: ms(3),
        window: WindowId::from_raw(1),
        event: WindowEvent::FocusLost,
    });
    system.update(ms(4));
    let phases: Vec<_> = seen(&system).iter().map(|(pointer, phase, _)| (*pointer, *phase)).collect();
    assert_eq!(
        phases,
        [(Pointer::Mouse, PointerPhase::Canceled), (Pointer::Finger(2), PointerPhase::Canceled)]
    );
    system.update(ms(5));
    assert_eq!(seen(&system), [], "and it's said once");
}

/// `push` is public, so a slot no adapter makes reaches it: past the slots there are, it's dropped
/// rather than shifted out of the held fingers' bits or reported as a finger nothing tracks.
#[test]
fn a_finger_past_the_slots_is_dropped() {
    let mut system = system();
    for finger in [MAX_FINGERS, 31, 32, usize::MAX] {
        for phase in [TouchPhase::Down, TouchPhase::Moved, TouchPhase::Up] {
            system.push(ms(1), RawInput::Touch { finger, phase, position: Vec2::ONE });
        }
    }
    system.push(
        ms(1),
        RawInput::Touch { finger: MAX_FINGERS - 1, phase: TouchPhase::Down, position: Vec2::ONE },
    );
    system.push(ms(2), RawInput::ReleaseAll);
    system.update(ms(3));
    assert_eq!(
        seen(&system),
        [
            (Pointer::Finger(MAX_FINGERS - 1), PointerPhase::Down, Vec2::ONE),
            (Pointer::Finger(MAX_FINGERS - 1), PointerPhase::Canceled, Vec2::ZERO),
        ]
    );
}
