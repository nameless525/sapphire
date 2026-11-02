//! The stick's path, from `sapphire_input`'s raw axes to the simulation's whole steps: dead zone,
//! sum, length limit, quantizing. A stick at the end of its range reads ±127, and a centred one
//! exactly 0. These read the stick as it was pushed (`Rig::next`); `world_stick.rs` has the camera's
//! turn.

mod support;

use sapphire_gameplay::{Buttons, Stick};
use sapphire_input::{PadAxis, RawInput};
use sapphire_math::Vec2;
use sapphire_platform::Scancode;
use support::{Rig, axis, key};

const LX: PadAxis = PadAxis::LeftX;
const LY: PadAxis = PadAxis::LeftY;

/// A stick at these SDL axes: +x right, +y down, each ±1 at the end of its range.
fn stick(x: f32, y: f32) -> Vec<RawInput> {
    vec![axis(LX, x), axis(LY, y)]
}

#[test]
fn the_path_from_axes_to_steps() {
    // The 45° reading: √½ × 127 rounds to 90.
    let root_half = std::f32::consts::FRAC_1_SQRT_2;
    let d = 90;
    let w = Scancode::W;
    let (s, a, e) = (Scancode::S, Scancode::A, Scancode::D);
    let keys = |held: &[Scancode]| held.iter().map(|&code| key(code, true)).collect::<Vec<_>>();
    let table: Vec<(&str, Vec<RawInput>, Stick)> = vec![
        ("never touched", vec![], Stick::CENTER),
        ("exactly centred", stick(0.0, 0.0), Stick::CENTER),
        (
            "SDL's resting noise, one count from the middle",
            stick(-1.0 / 32768.0, 1.0 / 32767.0),
            Stick::CENTER,
        ),
        ("worn, at rest inside the dead zone", stick(0.1, -0.1), Stick::CENTER),
        ("the dead zone's edge along an axis", stick(0.15, 0.0), Stick::CENTER),
        ("fully right", stick(1.0, 0.0), Stick::new(127, 0)),
        ("fully left, at SDL's most negative", stick(-1.0, 0.0), Stick::new(-127, 0)),
        ("fully forward, which SDL reports as negative", stick(0.0, -1.0), Stick::new(0, 127)),
        ("fully back", stick(0.0, 1.0), Stick::new(0, -127)),
        ("a stick that can't quite reach the end", stick(0.96, 0.0), Stick::new(127, 0)),
        ("the dead zone's other edge", stick(0.0, -0.95), Stick::new(0, 127)),
        ("halfway between the edges", stick(0.55, 0.0), Stick::new(64, 0)),
        ("a round gate's diagonal", stick(root_half, -root_half), Stick::new(d, d)),
        ("a square gate's corner runs no faster", stick(1.0, 1.0), Stick::new(d, -d)),
        ("just past the dead zone stays small", stick(0.16, 0.0), Stick::new(2, 0)),
        ("right, on a key", keys(&[e]), Stick::new(127, 0)),
        ("left, on a key", keys(&[a]), Stick::new(-127, 0)),
        ("forward, on a key", keys(&[w]), Stick::new(0, 127)),
        ("back, on a key", keys(&[s]), Stick::new(0, -127)),
        ("two keys' diagonal runs no faster", keys(&[w, e]), Stick::new(d, d)),
        ("two keys against each other cancel exactly", keys(&[a, e]), Stick::CENTER),
        ("all four cancel exactly", keys(&[w, s, a, e]), Stick::CENTER),
        (
            "a key and a stick add, held to the range",
            [keys(&[w]), stick(1.0, 0.0)].concat(),
            Stick::new(d, d),
        ),
        ("the right stick isn't movement", vec![axis(PadAxis::RightX, 1.0)], Stick::CENTER),
    ];

    let mut failures = Vec::new();
    for (name, input, want) in &table {
        let mut rig = Rig::new();
        rig.frame(input);
        let got = rig.next().stick;
        if got != *want {
            failures.push(format!("{name}: read {got:?}, expected {want:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_stick_let_go_is_exactly_zero_again() {
    let mut rig = Rig::new();
    rig.frame(&stick(1.0, -1.0));
    assert_ne!(rig.next().stick, Stick::CENTER);
    rig.frame(&stick(0.0, 0.0));
    assert_eq!(rig.next().stick, Stick::CENTER);
}

#[test]
fn every_step_of_a_frame_reads_the_frame_value() {
    let mut rig = Rig::new();
    rig.frame(&stick(1.0, 0.0));
    let steps: Vec<_> = (0..2).map(|_| rig.next().stick).collect();
    assert_eq!(steps, [Stick::new(127, 0); 2]);
    // A frame with no step moves on to the latest value: a stick is a level, not an event.
    rig.frame(&stick(0.0, -1.0));
    rig.frame(&stick(-1.0, 0.0));
    assert_eq!(rig.next().stick, Stick::new(-127, 0));
}

/// A key that comes and goes between steps is a move too, so one step reads it, and only one.
#[test]
fn a_move_too_quick_for_the_steps_reaches_one() {
    let (e, a) = (Scancode::D, Scancode::A);
    let (right, left) = (Stick::new(127, 0), Stick::new(-127, 0));
    const C: Stick = Stick::CENTER;
    type Frame = (Vec<RawInput>, Vec<Stick>);
    let table: [(&str, Vec<Frame>); 6] = [
        (
            "inside one frame, at one step a frame",
            vec![(vec![key(e, true), key(e, false)], vec![right]), (vec![], vec![C])],
        ),
        ("inside one frame, at two steps a frame", vec![(vec![key(e, true), key(e, false)], vec![right, C])]),
        (
            "in a frame with no step, it waits for the next",
            vec![(vec![key(e, true), key(e, false)], vec![]), (vec![], vec![right, C])],
        ),
        (
            "a level takes over from a tap",
            vec![(vec![key(e, true), key(e, false)], vec![]), (vec![key(a, true)], vec![left, left])],
        ),
        (
            "two keys against each other are no tap",
            vec![
                (vec![key(e, true), key(a, true)], vec![C, C]),
                (vec![key(e, false), key(a, false)], vec![C]),
            ],
        ),
        (
            "a key held over frames, then let go, is no tap",
            vec![(vec![key(e, true)], vec![right]), (vec![key(e, false)], vec![C, C])],
        ),
    ];
    let mut failures = Vec::new();
    for (name, frames) in table {
        let mut rig = Rig::new();
        for (index, (input, want)) in frames.into_iter().enumerate() {
            rig.frame(&input);
            let read: Vec<_> = want.iter().map(|_| rig.next().stick).collect();
            if read != want {
                failures.push(format!("{name}, frame {index}: read {read:?}, expected {want:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn on_screen_values_merge_with_the_pad() {
    let mut rig = Rig::new();
    let t = |ms: u64| support::ms(ms);
    rig.controls.set_on_screen(t(20), Buttons::NONE, Vec2::new(1.0, 0.0));
    rig.finish_frame();
    assert_eq!(rig.next().stick, Stick::new(127, 0), "a virtual stick at full tilt");

    // Forward on screen and right on the pad make the same diagonal as any other two.
    rig.controls.set_on_screen(t(40), Buttons::NONE, Vec2::new(0.0, 1.0));
    rig.controls.input_mut().push(t(41), axis(LX, 1.0));
    rig.finish_frame();
    assert_eq!(rig.next().stick, Stick::new(90, 90));

    rig.controls.set_on_screen(t(60), Buttons::NONE, Vec2::ZERO);
    rig.controls.input_mut().push(t(61), axis(LX, 0.0));
    rig.finish_frame();
    assert_eq!(rig.next().stick, Stick::CENTER, "exactly centred again");
}

#[test]
fn on_screen_buttons_press_like_any_other() {
    let t = |ms: u64| support::ms(ms);
    let mut rig = Rig::new();
    // A tap that came and went inside one frame, as its zone saw it.
    rig.controls.set_on_screen(t(20), Buttons::SKILL | Buttons::JUMP, Vec2::ZERO);
    rig.controls.set_on_screen(t(25), Buttons::JUMP, Vec2::ZERO);
    rig.finish_frame();
    assert_eq!(rig.steps(2), [Buttons::SKILL | Buttons::JUMP, Buttons::JUMP]);

    rig.controls.set_on_screen(t(40), Buttons::NONE, Vec2::ZERO);
    rig.finish_frame();
    assert_eq!(rig.steps(1), [Buttons::NONE]);

    // Losing focus lets go of on-screen controls too.
    rig.controls.set_on_screen(t(60), Buttons::DODGE, Vec2::new(1.0, 0.0));
    rig.finish_frame();
    assert_eq!(rig.steps(1), [Buttons::DODGE]);
    rig.focus_lost(1, &[]);
    rig.finish_frame();
    let next = rig.next();
    assert_eq!((next.buttons, next.stick), (Buttons::NONE, Stick::CENTER));
}

#[test]
fn look_and_zoom_are_the_apps_and_not_the_ticks() {
    let mut rig = Rig::new();
    assert_eq!(rig.controls.look(), Vec2::ZERO, "exactly still at rest");
    rig.frame(&[axis(PadAxis::RightX, 1.0)]);
    let look = rig.controls.look();
    assert!((look.x - -2.5 * 0.016).abs() < 1e-5 && look.y == 0.0, "2.5 rad/s at full tilt: {look}");
    assert_eq!(rig.next(), sapphire_gameplay::ActorActions::NEUTRAL);

    // The curve: half the tilt past the dead zone is a quarter of the rate.
    rig.frame(&[axis(PadAxis::RightX, 0.55)]);
    let half = rig.controls.look();
    assert!((half.x - -2.5 * 0.25 * 0.016).abs() < 1e-5, "{half}");

    rig.frame(&[
        axis(PadAxis::RightX, 0.0),
        RawInput::MouseWheel { delta: Vec2::new(0.0, 1.0) },
        RawInput::MouseMotion { delta: Vec2::new(10.0, 0.0) },
    ]);
    assert!((rig.controls.camera_zoom().exp() - 0.9).abs() < 1e-5, "0.9× the distance per notch");
    assert!((rig.controls.look().x - -0.08).abs() < 1e-5, "0.008 rad per point");
    assert_eq!(rig.next(), sapphire_gameplay::ActorActions::NEUTRAL);
}
