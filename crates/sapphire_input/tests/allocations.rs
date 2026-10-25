//! Steady-state frames allocate nothing: device input of every kind, triggers with deadlines,
//! chords, gestures and the update itself. Installs `CountingAllocator` as this test binary's
//! global allocator.

use std::alloc::System;

use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_input::{InputMap, InputSystem, PadAxis, PadButton, RawInput, Timestamp};
use sapphire_math::Vec2;
use sapphire_platform::{GamepadType, Scancode, TouchPhase};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

const BINDINGS: &str = r#"(type: "sapphire::input::InputBindings", version: 1, data: (
    actions: [
        (name: "move", kind: Axis2D), (name: "look", kind: Axis2D), (name: "zoom", kind: Axis1D),
        (name: "grab", kind: Button), (name: "attack", kind: Button), (name: "heavy", kind: Button),
        (name: "dash", kind: Button), (name: "dodge", kind: Button), (name: "guard", kind: Button),
    ],
    contexts: [
        (name: "combat", bindings: [
            (action: "move", source: Key(W), modifiers: [Swizzle, ScaleByDeltaTime]),
            (action: "move", source: PadStick(Left), modifiers: [DeadZone(), ResponseCurve(exponent: 2.0)]),
            (action: "look", source: MouseMotion, chord: "grab", modifiers: [Scale((0.01, 0.01))]),
            (action: "look", source: Touch(Drag)),
            (action: "zoom", source: Touch(Pinch)),
            (action: "zoom", source: MouseWheel),
            (action: "grab", source: Mouse(Right)),
            (action: "attack", source: Key(J), trigger: Tap(seconds: 0.2)),
            (action: "heavy", source: Key(J), trigger: Hold(seconds: 0.4)),
            (action: "dash", source: Key(K), trigger: DoubleTap(seconds: 0.3)),
            (action: "dodge", source: Touch(Swipe(Right)), trigger: Pressed),
            (action: "guard", source: Pad(LeftShoulder), trigger: Released),
        ]),
        (name: "menu", bindings: [(action: "attack", source: Key(Return), trigger: Pressed)]),
    ],
))"#;

/// One 16 ms frame starting at `frame`'s time, with input from every device.
fn frame(input: &mut InputSystem, frame: u64) {
    let t = |offset: u64| Timestamp::from_nanos((frame * 16 + offset) * 1_000_000);
    let down = !frame.is_multiple_of(3);
    input.push(t(1), RawInput::Key { scancode: Scancode::W, down });
    input.push(t(2), RawInput::Key { scancode: Scancode::J, down: frame % 40 < 30 });
    input.push(t(3), RawInput::Key { scancode: Scancode::K, down: frame % 4 < 2 });
    input.push(t(4), RawInput::MouseButton { button: sapphire_input::MouseButton::Right, down });
    input.push(t(5), RawInput::MouseMotion { delta: Vec2::new(2.0, -1.0) });
    input.push(t(6), RawInput::MouseWheel { delta: Vec2::new(0.0, 1.0) });
    let stick = (frame as f32 * 0.1).sin();
    input.push(t(7), RawInput::PadAxis { pad: 0, axis: PadAxis::LeftX, value: stick });
    input.push(t(8), RawInput::PadButton { pad: 0, button: PadButton::LeftShoulder, down });
    let x = 100.0 + (frame % 10) as f32 * 30.0;
    let phase = match frame % 10 {
        0 => TouchPhase::Down,
        9 => TouchPhase::Up,
        _ => TouchPhase::Moved,
    };
    input.push(t(9), RawInput::Touch { finger: 0, phase, position: Vec2::new(x, 200.0) });
    let second = match frame % 20 {
        3 => Some(TouchPhase::Down),
        6 => Some(TouchPhase::Up),
        4 | 5 => Some(TouchPhase::Moved),
        _ => None,
    };
    if let Some(phase) = second {
        input.push(t(10), RawInput::Touch { finger: 1, phase, position: Vec2::new(500.0 + x, 300.0) });
    }
    input.update(t(16));
}

#[test]
fn steady_state_frames_do_not_allocate() {
    // Counts are exact per thread only where Rust uses native TLS (see `sapphire_core::alloc`).
    if !PER_THREAD_COUNTS {
        return;
    }
    let mut input = InputSystem::new(InputMap::from_ron(BINDINGS).expect("valid bindings"));
    let combat = input.map().context("combat").expect("combat");
    input.enable_context(combat, 0);
    input.push(Timestamp::default(), RawInput::GamepadAdded { pad: 0, kind: GamepadType::XboxOne });
    for n in 0..60 {
        frame(&mut input, n);
    }

    let mut events = 0;
    let ((), counts) = alloc::count_in_scope(|| {
        for n in 60..1060 {
            frame(&mut input, n);
            events += input.events().len();
            // Context changes rebuild into preallocated lists too.
            if n.is_multiple_of(100) {
                let menu = input.map().context("menu").expect("menu");
                if input.is_enabled(menu) {
                    input.disable_context(menu);
                } else {
                    input.enable_context(menu, 10);
                }
            }
        }
    });
    assert!(alloc::is_installed());
    assert!(events > 1000, "the frames exercised the bindings: {events} events");
    assert_eq!(counts.allocations, 0, "1000 frames allocated {} times", counts.allocations);
}
