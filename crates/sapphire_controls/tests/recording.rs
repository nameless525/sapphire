//! The bindings as a replay's source (`sapphire_gameplay::replay::ActionSource`): a session someone
//! plays is recorded, and plays back to the same checksums with no input and no camera.

mod support;

use std::f32::consts::FRAC_PI_2;

use sapphire_gameplay::replay::{ActionSource, Player, Recording, Replay, SCENARIOS};
use sapphire_gameplay::{ActorActions, ActorId, Buttons, Stick, TickActions};
use sapphire_platform::Scancode;
use support::{Rig, key};

const J: Scancode = Scancode::J;
const W: Scancode = Scancode::W;

fn id(raw: u32) -> ActorId {
    ActorId::from_raw(raw).expect("not zero")
}

/// What these frames record under a camera with this yaw, one step a frame.
fn record(camera_yaw: f32, frames: &[(Scancode, bool)]) -> (Replay, sapphire_gameplay::replay::Played) {
    let scenario = &SCENARIOS[0];
    let mut recording = Recording::new(scenario, scenario.config).expect("sets up");
    let mut rig = Rig::new();
    for &(code, down) in frames {
        rig.frame(&[key(code, down)]);
        recording.step(&mut rig.controls.source(camera_yaw)).expect("under the limit");
    }
    recording.finish()
}

const SESSION: [(Scancode, bool); 6] = [(W, true), (J, true), (J, false), (W, false), (J, true), (J, false)];

#[test]
fn a_played_session_plays_back_to_the_same_trail() {
    let (replay, recorded) = record(0.0, &SESSION);
    assert_eq!(replay.ticks(), SESSION.len() as u32);

    let decoded = Replay::decode(&replay.encode()).expect("reads back");
    let mut player = Player::new(decoded, SCENARIOS).expect("starts");
    player.run_to_end();
    assert_eq!(player.finish().trail, recorded.trail);
}

/// The player is the cast's first, everyone else is neutral, and the file holds world-space values:
/// the direction the player meant, with no camera left in it.
#[test]
fn the_file_holds_the_direction_the_player_meant() {
    let cast = SCENARIOS[0].start(SCENARIOS[0].config).expect("sets up").1;
    let player = cast[0];
    let actions = |replay: &Replay| -> Vec<TickActions> { replay.actions().collect() };

    let (along_z, _) = record(0.0, &SESSION);
    let (along_x, _) = record(FRAC_PI_2, &SESSION);
    let forward = |stick: Stick| ActorActions { buttons: Buttons::NONE, stick };
    let press = |stick: Stick| ActorActions { buttons: Buttons::LIGHT, stick };
    let read = |replay: &Replay| -> Vec<ActorActions> {
        actions(replay).iter().map(|tick| tick.get(player)).collect()
    };
    let (z, x) = (Stick::new(0, 127), Stick::new(127, 0));
    assert_eq!(
        read(&along_z),
        [
            forward(z),
            press(z),
            forward(z),
            ActorActions::NEUTRAL,
            press(Stick::CENTER),
            ActorActions::NEUTRAL
        ]
    );
    assert_eq!(
        read(&along_x),
        [
            forward(x),
            press(x),
            forward(x),
            ActorActions::NEUTRAL,
            press(Stick::CENTER),
            ActorActions::NEUTRAL
        ]
    );
    for tick in actions(&along_z) {
        assert!(cast[1..].iter().all(|&other| tick.get(other) == ActorActions::NEUTRAL));
    }
}

/// `Recording::step` asks once a step, and that's what moves a press along.
#[test]
fn asking_the_source_is_reading_the_step() {
    let mut rig = Rig::new();
    rig.frame(&[key(J, true), key(J, false), key(J, true), key(J, false)]);
    let player = id(9);
    let mut source = rig.controls.source(0.0);
    let read = [
        source.actions(0, &[player, id(10)]),
        source.actions(1, &[player, id(10)]),
        source.actions(2, &[player, id(10)]),
        source.actions(3, &[player, id(10)]),
    ];
    let buttons = read.map(|tick| tick.get(player).buttons);
    let (l, n) = (Buttons::LIGHT, Buttons::NONE);
    assert_eq!(buttons, [l, n, l, n], "two presses, each with its edge");
    assert!(read.iter().all(|tick| tick.get(id(10)) == ActorActions::NEUTRAL));
}

/// With no one to hand it to, a step still passes: otherwise a press would wait for someone to come
/// along, and land on them.
#[test]
fn a_cast_of_no_one_still_moves_a_press_along() {
    let mut rig = Rig::new();
    rig.frame(&[key(J, true), key(J, false)]);
    let tick = rig.controls.source(0.0).actions(0, &[]);
    assert_eq!(tick, TickActions::NEUTRAL);
    assert_eq!(rig.steps(1), [Buttons::NONE], "the press went with that step");
}

/// A pause records nothing: no steps run, so the file has no ticks for it, and the fight picks up
/// where it stopped.
#[test]
fn a_pause_leaves_no_ticks_in_the_file() {
    let scenario = &SCENARIOS[0];
    let mut recording = Recording::new(scenario, scenario.config).expect("sets up");
    let mut driver = sapphire_time::FixedStepDriver::default();
    let mut rig = Rig::new();
    let frame = std::time::Duration::from_millis(16);
    let mut steps_taken = 0;
    let mut run = |rig: &mut Rig, input: &[sapphire_input::RawInput]| {
        rig.frame(input);
        let steps = rig.controls.advance(&mut driver, frame);
        for _ in 0..steps.count {
            recording.step(&mut rig.controls.source(0.0)).expect("under the limit");
            steps_taken += 1;
        }
    };
    let esc = Scancode::ESCAPE;
    for _ in 0..30 {
        run(&mut rig, &[]);
    }
    let before = 30;
    run(&mut rig, &[key(esc, true)]);
    run(&mut rig, &[key(esc, false)]);
    assert!(rig.controls.is_paused());
    for _ in 0..300 {
        run(&mut rig, &[key(J, true), key(J, false)]);
    }
    run(&mut rig, &[key(esc, true)]);
    run(&mut rig, &[key(esc, false)]);
    assert!(!rig.controls.is_paused());
    for _ in 0..30 {
        run(&mut rig, &[]);
    }
    let (replay, played) = recording.finish();
    assert_eq!(replay.ticks(), steps_taken);
    assert_eq!(played.stats.steps, u64::from(steps_taken));
    assert!(steps_taken < before + 30 + 5, "five minutes of pause took no steps: {steps_taken}");
    assert!(
        replay.actions().all(|tick| tick.is_empty()),
        "and none of the presses made during it reached the file"
    );
}
