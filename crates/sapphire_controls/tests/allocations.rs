//! Steady-state frames allocate nothing: presses, steps, pauses, on-screen values and fingers. It runs on the
//! same frames as the fixed step, whose rule R6 the mapping's half must not break. Installs
//! `CountingAllocator` as this test binary's global allocator.

use std::alloc::System;

use sapphire_controls::CombatControls;
use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_gameplay::replay::ActionSource;
use sapphire_gameplay::{ActorId, Buttons};
use sapphire_input::{PadAxis, PadButton, RawInput, Timestamp};
use sapphire_math::Vec2;
use sapphire_platform::{GamepadType, Scancode};
use sapphire_time::FixedStepDriver;
use sapphire_ui::{Button, ButtonEvent, ButtonEventKind, StickState, TouchActions};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

/// What the zones say of a finger on the attack button and a thumb on the stick, on and off, as the
/// app's frame reads them. `game_ui` holds the zones to the same, with its own allocation test.
fn fingers(touch: &mut TouchActions, controls: &mut CombatControls, frame: u64) {
    let t = |offset: u64| Timestamp::from_nanos((frame * 16 + offset) * 1_000_000);
    let attack = |kind, at| ButtonEvent { button: Button::Attack, kind, at, drag: Vec2::ZERO };
    touch.events.clear();
    match frame % 16 {
        0 => {
            touch.events.push(attack(ButtonEventKind::Pressed, t(8)));
            touch.stick = StickState { active: true, ..StickState::default() };
        }
        4 => touch.stick.value = Vec2::X,
        8 => {
            touch.events.push(attack(ButtonEventKind::Released, t(8)));
            touch.stick = StickState::default();
        }
        _ => {}
    }
    controls.read_touch(touch, t(10));
}

/// One 16 ms frame starting at `frame`'s time, with input from every device and a pause now and
/// then.
fn frame(
    controls: &mut CombatControls,
    touch: &mut TouchActions,
    driver: &mut FixedStepDriver,
    frame: u64,
) -> u64 {
    let t = |offset: u64| Timestamp::from_nanos((frame * 16 + offset) * 1_000_000);
    let down = frame % 4 < 2;
    let input = controls.input_mut();
    input.push(t(1), RawInput::Key { scancode: Scancode::J, down });
    input.push(t(2), RawInput::Key { scancode: Scancode::W, down: !down });
    input.push(t(3), RawInput::PadButton { pad: 0, button: PadButton::South, down });
    let stick = (frame as f32 * 0.1).sin();
    input.push(t(4), RawInput::PadAxis { pad: 0, axis: PadAxis::LeftX, value: stick });
    input.push(t(5), RawInput::PadAxis { pad: 0, axis: PadAxis::RightX, value: stick });
    // Every 100 frames, the pause opens or closes.
    input.push(t(6), RawInput::Key { scancode: Scancode::ESCAPE, down: frame.is_multiple_of(100) });
    let buttons = if frame % 8 < 4 { Buttons::DODGE } else { Buttons::NONE };
    controls.set_on_screen(t(7), buttons, Vec2::new(stick, 0.0));
    fingers(touch, controls, frame);
    controls.update(t(16));
    let mut seen = 0;
    let steps = controls.advance(driver, std::time::Duration::from_millis(33));
    let player = ActorId::from_raw(1).expect("not zero");
    // Under a turning camera, and through the source a recording asks, half the time.
    let camera_yaw = frame as f32 * 0.05;
    for step in 0..steps.count {
        let tick = if (frame + u64::from(step)).is_multiple_of(2) {
            controls.next_tick(player, camera_yaw)
        } else {
            controls.source(camera_yaw).actions(frame, &[player])
        };
        seen += tick.get(player).buttons.bits().count_ones() as u64;
    }
    seen + u64::from(controls.look() != Vec2::ZERO)
        + u64::from(controls.camera_zoom() != 0.0)
        + u64::from(controls.raw_stick() != Vec2::ZERO)
}

#[test]
fn steady_state_frames_do_not_allocate() {
    // Counts are exact per thread only where Rust uses native TLS (see `sapphire_core::alloc`).
    if !PER_THREAD_COUNTS {
        return;
    }
    let mut controls = CombatControls::with_default_bindings().expect("the default bindings");
    let mut driver = FixedStepDriver::default();
    let mut touch = TouchActions { events: Vec::with_capacity(1), ..TouchActions::default() };
    let pad = RawInput::GamepadAdded { pad: 0, kind: GamepadType::XboxOne };
    controls.input_mut().push(Timestamp::default(), pad);
    for n in 0..120 {
        frame(&mut controls, &mut touch, &mut driver, n);
    }

    let mut seen = 0;
    let ((), counts) = alloc::count_in_scope(|| {
        for n in 120..1120 {
            seen += frame(&mut controls, &mut touch, &mut driver, n);
        }
    });
    assert!(alloc::is_installed());
    assert!(seen > 500, "the frames exercised the mapping: {seen}");
    assert!(controls.stats().presses > 100, "and pressed things: {:?}", controls.stats());
    assert_eq!(counts.allocations, 0, "1000 frames allocated {} times", counts.allocations);
}
