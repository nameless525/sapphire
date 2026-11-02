//! A tick's stick is world-space: the mapping turns it by the camera's yaw before quantizing, so the
//! simulation never reads a camera and a replay carries the direction the player meant.

mod support;

use std::f32::consts::{FRAC_1_SQRT_2, FRAC_PI_2, FRAC_PI_4, PI};

use sapphire_gameplay::{Buttons, Stick};
use sapphire_input::{PadAxis, RawInput, Timestamp};
use sapphire_math::{Vec2, Vec3, det};
use sapphire_platform::Scancode;
use support::{Rig, axis, key};

const W: Scancode = Scancode::W;
const D: Scancode = Scancode::D;

/// How far a read may be from a true length: half a step per axis, through `Stick::to_vec2`.
const ROUNDING: f32 = 0.5 * std::f32::consts::SQRT_2 / 127.0;

/// A rig with this held on-screen, which has no dead zone between it and the mapping.
fn pushed(stick: Vec2) -> Rig {
    let mut rig = Rig::new();
    let now = Timestamp::from_nanos(1_000_000);
    rig.controls.set_on_screen(now, Buttons::NONE, stick);
    rig.finish_frame();
    rig
}

/// The camera's right and forward, read off the convention rather than the code: forward is an
/// actor's facing at that yaw, and right is forward × up in a right-handed world.
fn view(camera_yaw: f32) -> (Vec3, Vec3) {
    let forward = det::direction_from_yaw(camera_yaw);
    (forward.cross(Vec3::Y), forward)
}

#[test]
fn each_quarter_turn_moves_the_stick_round() {
    let (up, down, right, left) =
        (Stick::new(0, 127), Stick::new(0, -127), Stick::new(127, 0), Stick::new(-127, 0));
    // (the camera's yaw, what forward reads, what right reads). In the stick's own terms, x is world
    // +X and y is world +Z.
    let table = [
        ("along +Z, whose right is −X", 0.0, up, left),
        ("along +X, whose right is +Z", FRAC_PI_2, right, up),
        ("along −Z, whose right is +X", PI, down, right),
        ("along −X, whose right is −Z", -FRAC_PI_2, left, down),
        ("a turn and a quarter", FRAC_PI_2 + 2.0 * PI, right, up),
    ];
    let mut failures = Vec::new();
    for (name, yaw, forward, sideways) in table {
        for (key, want) in [(W, forward), (D, sideways)] {
            let mut rig = Rig::new();
            rig.frame(&[self::key(key, true)]);
            let got = rig.controls.next_actions(yaw).stick;
            if got != want {
                failures.push(format!("{name}, {key:?}: read {got:?}, expected {want:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// What the docs tell a camera to pass: `det::yaw_of_direction` of its forward vector, tilted or not.
/// Forward then walks along the ground under the view, and right walks to its right.
#[test]
fn a_cameras_forward_vector_gives_its_yaw() {
    for forward in [
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(0.0, 0.0, -1.0),
        Vec3::new(1.0, -0.3, 0.0),
        Vec3::new(-0.6, -0.5, 0.62),
        Vec3::new(-0.2, 0.4, -0.9),
    ] {
        let yaw = det::yaw_of_direction(forward);
        let ground = Vec3::new(forward.x, 0.0, forward.z).normalize();
        let right = forward.normalize().cross(Vec3::Y).normalize();
        for (code, want) in [(W, ground), (D, right)] {
            let mut rig = Rig::new();
            rig.frame(&[key(code, true)]);
            let read = rig.controls.next_actions(yaw).stick.to_vec2();
            let error = Vec2::new(want.x, want.z).distance(read);
            assert!(error <= ROUNDING + 1e-5, "{code:?} under {forward}: read {read}, off by {error}");
        }
    }
}

/// The rule for a fully tilted diagonal: the length limit comes before the turn, so nothing clips and
/// it stays the speed it was, wherever the camera points.
#[test]
fn a_fully_tilted_diagonal_stays_one_speed_under_any_camera() {
    let keys = [key(W, true), key(D, true)];
    let corner = [axis(PadAxis::LeftX, 1.0), axis(PadAxis::LeftY, -1.0)];
    let table = [
        ("along +Z, right of forward is +Z and −X", 0.0, Stick::new(-90, 90)),
        ("along −Z", PI, Stick::new(90, -90)),
        ("turned so forward-right is +Z", FRAC_PI_4, Stick::new(0, 127)),
        ("turned so forward-right is −X", -FRAC_PI_4, Stick::new(-127, 0)),
    ];
    for (name, yaw, want) in table {
        for (source, input) in [("two keys", &keys), ("a square gate's corner", &corner)] {
            let mut rig = Rig::new();
            rig.frame(input);
            assert_eq!(rig.controls.next_actions(yaw).stick, want, "{source}, {name}");
        }
    }
}

/// Every yaw, every direction and every length quantizes in range, keeps its direction and runs no
/// faster than a full tilt, give or take the rounding of each axis.
#[test]
fn every_push_under_every_camera_quantizes_in_range() {
    let mut rig = Rig::new();
    let mut now = 1_000_000;
    let mut read_longest: f32 = 0.0;
    for yaw_step in 0..360 {
        // Past a few turns too, since a camera's yaw needn't be wrapped.
        let yaw = (yaw_step as f32 - 180.0) * 0.0873;
        for tilt_step in 0..48 {
            let angle = tilt_step as f32 * std::f32::consts::TAU / 48.0;
            for length in [0.3, 0.75, 1.0, 1.4] {
                // On a square gate an on-screen stick may read past a length of 1, as a corner does.
                let tilt = det::vec2_from_angle(angle) * length;
                now += 1_000;
                rig.controls.set_on_screen(Timestamp::from_nanos(now), Buttons::NONE, tilt);
                now += 1_000;
                rig.controls.update(Timestamp::from_nanos(now));

                let stick = rig.controls.next_actions(yaw).stick;
                assert!(stick.x() >= -127 && stick.y() >= -127, "{tilt} at {yaw}: {stick:?}");
                let read = stick.to_vec2();
                read_longest = read_longest.max(read.length());

                let (right, forward) = view(yaw);
                let limited = tilt.clamp_length_max(1.0);
                let world = right * limited.x + forward * limited.y;
                let error = Vec2::new(world.x, world.z).distance(read);
                assert!(error <= ROUNDING + 1e-5, "{tilt} at {yaw}: read {read}, off by {error}");
            }
        }
    }
    assert!(read_longest > 1.0, "the rounding the rule states does happen: {read_longest}");
    assert!(read_longest <= 1.0 + ROUNDING + 1e-5, "{read_longest}");
}

#[test]
fn a_stick_at_rest_is_exactly_zero_under_every_camera() {
    let mut rig = Rig::new();
    for yaw in [0.0, 1.0, -2.5, PI, 1e6, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(rig.controls.next_actions(yaw).stick, Stick::CENTER, "{yaw}");
    }
    rig.frame(&[axis(PadAxis::LeftX, 0.1), axis(PadAxis::LeftY, -0.1)]);
    assert_eq!(rig.controls.next_actions(0.7).stick, Stick::CENTER, "inside the dead zone");
}

#[test]
fn a_camera_with_no_yaw_moves_no_one() {
    let mut rig = pushed(Vec2::new(0.6, 0.8));
    for yaw in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(rig.controls.next_actions(yaw).stick, Stick::CENTER, "{yaw}");
    }
    assert_ne!(rig.controls.next_actions(0.0).stick, Stick::CENTER, "a good one does again");
}

/// Steps read the camera they're given, so a camera that turned between two reads the new direction.
#[test]
fn each_step_reads_the_yaw_it_is_given() {
    let mut rig = Rig::new();
    rig.frame(&[key(W, true)]);
    assert_eq!(rig.controls.next_actions(0.0).stick, Stick::new(0, 127));
    assert_eq!(rig.controls.next_actions(FRAC_PI_2).stick, Stick::new(127, 0));
    assert_eq!(rig.controls.next_actions(PI).stick, Stick::new(0, -127));
}

/// A tap goes by the camera that was there when a step read it.
#[test]
fn a_tap_turns_too() {
    let mut rig = Rig::new();
    rig.frame(&[key(D, true), key(D, false)]);
    assert_eq!(rig.controls.next_actions(FRAC_PI_2).stick, Stick::new(0, 127));
    assert_eq!(rig.controls.next_actions(FRAC_PI_2).stick, Stick::CENTER, "and only once");
}

/// The raw stick is what the camera's own input reads: as pushed, and never turned.
#[test]
fn the_raw_stick_knows_no_camera() {
    let mut rig = Rig::new();
    assert_eq!(rig.controls.raw_stick(), Vec2::ZERO, "exactly still at rest");

    rig.frame(&[key(W, true), key(D, true)]);
    let limited = Vec2::splat(FRAC_1_SQRT_2);
    assert!(rig.controls.raw_stick().distance(limited) < 1e-5, "length limited");
    let _ = rig.controls.next_actions(1.0);
    assert!(rig.controls.raw_stick().distance(limited) < 1e-5, "and a step leaves it alone");

    rig.frame(&[key(W, false), key(D, false)]);
    assert_eq!(rig.controls.raw_stick(), Vec2::ZERO, "exactly zero again");

    // +x right, +y forward, and a pad's y flipped on the way.
    rig.frame(&[axis(PadAxis::LeftY, -1.0)]);
    assert_eq!(rig.controls.raw_stick(), Vec2::Y);
    rig.frame(&[axis(PadAxis::LeftY, 0.0), axis(PadAxis::LeftX, 1.0)]);
    assert_eq!(rig.controls.raw_stick(), Vec2::X);

    // It's the move action's, so the pause's context has none of it.
    rig.frame(&[axis(PadAxis::LeftX, 1.0), key(Scancode::ESCAPE, true)]);
    assert!(rig.controls.is_paused());
    rig.frame(&[key(Scancode::ESCAPE, false)]);
    let raw: [RawInput; 0] = [];
    rig.frame(&raw);
    assert_eq!(rig.controls.raw_stick(), Vec2::ZERO, "the stick is the game's, while it's paused");
}
