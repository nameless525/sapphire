//! The pause: it stops the steps and only the steps, resuming takes no burst, and the simulation
//! can't tell it happened.

mod support;

use std::time::Duration;

use sapphire_controls::CombatControls;
use sapphire_gameplay::replay::{SCENARIOS, find_scenario};
use sapphire_gameplay::{ActorActions, ActorId, Simulation, Stick, TickActions};
use sapphire_input::{PadAxis, PadButton, RawInput};
use sapphire_platform::{Scancode, TouchPhase};
use sapphire_time::{FixedStepConfig, FixedStepDriver};
use support::{BACK, H, L, N, Rig, axis, key, pad};

const ESCAPE: Scancode = Scancode::ESCAPE;

fn tap(scancode: Scancode) -> [RawInput; 2] {
    [key(scancode, true), key(scancode, false)]
}

const FRAME_TIMES: [Duration; 4] = [
    Duration::from_nanos(8_333_333),
    Duration::from_nanos(16_666_667),
    Duration::from_nanos(33_333_333),
    Duration::from_millis(50),
];

/// Frames run, as the driver steps them, must follow a driver that never saw the pause, count for
/// count and alpha for alpha, whatever the frames' length.
#[test]
fn resuming_takes_no_burst() {
    for frame_time in FRAME_TIMES {
        let config = FixedStepConfig::default();
        let (mut driver, mut reference) = (FixedStepDriver::new(config), FixedStepDriver::new(config));
        let mut rig = Rig::new();

        for frame in 0..120 {
            rig.frame(&[]);
            let (steps, expected) =
                (rig.controls.advance(&mut driver, frame_time), reference.advance(frame_time));
            assert_eq!(steps, expected, "{frame_time:?}: frame {frame}");
        }

        let alpha = driver.alpha();
        let ticks = driver.ticks();
        rig.frame(&tap(ESCAPE));
        assert!(rig.controls.is_paused());
        // A minute of frames, and one long one.
        for frame in 0..3600 {
            rig.frame(&[]);
            let frame_time = if frame == 1800 { Duration::from_secs(30) } else { frame_time };
            let steps = rig.controls.advance(&mut driver, frame_time);
            assert_eq!(steps.count, 0, "{frame_time:?}: frame {frame}");
            assert_eq!(steps.alpha, alpha, "{frame_time:?}: frame {frame}: presentation holds still");
            assert_eq!(steps.dropped, Duration::ZERO, "{frame_time:?}: frame {frame}: no time lost");
        }
        assert_eq!(driver.ticks(), ticks);

        rig.frame(&tap(ESCAPE));
        rig.frame(&[]);
        assert!(!rig.controls.is_paused());
        for frame in 0..120 {
            let (steps, expected) =
                (rig.controls.advance(&mut driver, frame_time), reference.advance(frame_time));
            assert_eq!(steps, expected, "{frame_time:?}: frame {frame} after the pause");
            rig.frame(&[]);
        }
    }
}

#[test]
fn every_way_in_and_out_works() {
    let paused = |rig: &Rig| rig.controls.is_paused();
    let table: [(&str, Vec<RawInput>, Vec<RawInput>); 4] = [
        ("Esc", tap(ESCAPE).to_vec(), tap(ESCAPE).to_vec()),
        ("P", tap(Scancode::P).to_vec(), tap(Scancode::P).to_vec()),
        (
            "Start",
            vec![pad(PadButton::Start, true), pad(PadButton::Start, false)],
            vec![pad(PadButton::Start, true), pad(PadButton::Start, false)],
        ),
        (
            "Back, then East, as a phone with a pad does",
            tap(BACK).to_vec(),
            vec![pad(PadButton::East, true), pad(PadButton::East, false)],
        ),
    ];
    for (name, into, out) in table {
        let mut rig = Rig::new();
        assert!(!paused(&rig), "{name}");
        rig.frame(&into);
        assert!(paused(&rig), "{name}: the frame it's pressed in");
        rig.frame(&[]);
        assert!(paused(&rig), "{name}");
        rig.frame(&out);
        assert!(paused(&rig), "{name}: until the combat context is back");
        rig.frame(&[]);
        assert!(!paused(&rig), "{name}");
        assert!(!rig.controls.wants_to_leave(), "{name}");
    }

    // A tap anywhere is a phone's way back. Dragging isn't.
    let touch = |phase, x| RawInput::Touch { finger: 0, phase, position: sapphire_math::Vec2::new(x, 100.0) };
    let mut rig = Rig::new();
    rig.frame(&tap(BACK));
    rig.frame(&[
        touch(TouchPhase::Down, 100.0),
        touch(TouchPhase::Moved, 300.0),
        touch(TouchPhase::Up, 300.0),
    ]);
    rig.frame(&[]);
    assert!(paused(&rig), "a drag isn't a tap");
    rig.frame(&[touch(TouchPhase::Down, 100.0), touch(TouchPhase::Up, 100.0)]);
    rig.frame(&[]);
    assert!(!paused(&rig), "a tap resumes");
}

#[test]
fn android_back_opens_the_pause_then_leaves() {
    /// `(input, paused after, leaving after)`.
    type Frame = (Vec<RawInput>, bool, bool);
    let table: [(&str, Vec<Frame>); 6] = [
        (
            "Back, Back",
            vec![(tap(BACK).to_vec(), true, false), (vec![], true, false), (tap(BACK).to_vec(), true, true)],
        ),
        (
            "held down, it doesn't go on to leave",
            vec![
                (vec![key(BACK, true)], true, false),
                (vec![], true, false),
                (vec![], true, false),
                (vec![key(BACK, false)], true, false),
                (vec![key(BACK, true)], true, true),
            ],
        ),
        (
            "two Backs inside a frame only pause: the pause context isn't up yet",
            vec![
                (vec![key(BACK, true), key(BACK, false), key(BACK, true), key(BACK, false)], true, false),
                (vec![], true, false),
            ],
        ),
        (
            "resumed, Back pauses again before it leaves",
            vec![
                (tap(BACK).to_vec(), true, false),
                (tap(ESCAPE).to_vec(), true, false),
                (vec![], false, false),
                (tap(BACK).to_vec(), true, false),
                (tap(BACK).to_vec(), true, true),
            ],
        ),
        (
            "the pad's Back, as a phone's",
            vec![
                (vec![pad(PadButton::Back, true), pad(PadButton::Back, false)], true, false),
                (vec![pad(PadButton::Back, true), pad(PadButton::Back, false)], true, true),
            ],
        ),
        (
            "Backspace, on a keyboard without a Back",
            vec![
                (tap(Scancode::BACKSPACE).to_vec(), true, false),
                (tap(Scancode::BACKSPACE).to_vec(), true, true),
            ],
        ),
    ];
    let mut failures = Vec::new();
    for (name, frames) in table {
        let mut rig = Rig::new();
        for (index, (input, paused, leaving)) in frames.into_iter().enumerate() {
            rig.frame(&input);
            let got = (rig.controls.is_paused(), rig.controls.wants_to_leave());
            if got != (paused, leaving) {
                failures.push(format!(
                    "{name}, frame {index}: (paused, leaving) {got:?}, expected {:?}",
                    (paused, leaving)
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));

    // No verb leaves: a thumb that goes on mashing through a pause can't end the fight.
    let mut rig = Rig::new();
    rig.frame(&tap(ESCAPE));
    let verbs = [Scancode::J, Scancode::K, Scancode::LEFT_SHIFT, Scancode::E, Scancode::Q, Scancode::TAB];
    for verb in verbs {
        rig.frame(&tap(verb));
    }
    for button in [PadButton::West, PadButton::East, PadButton::South, PadButton::North] {
        rig.frame(&[pad(button, true), pad(button, false)]);
    }
    assert!(!rig.controls.wants_to_leave());
}

#[test]
fn presses_keep_to_their_side_of_the_pause() {
    let esc = tap(ESCAPE);

    // Before the pause, in the same frame: it arrives, on the first step after, as if no time passed.
    let mut rig = Rig::new();
    rig.frame(&[key(Scancode::J, true), key(Scancode::J, false), esc[0], esc[1]]);
    assert!(rig.controls.is_paused());
    rig.frame(&[]);
    rig.frame(&esc);
    rig.frame(&[]);
    assert!(!rig.controls.is_paused());
    assert_eq!(rig.steps(2), [L, N]);

    // After the pause, in the same frame: it's the pause's, however many steps come later.
    let mut rig = Rig::new();
    rig.frame(&[esc[0], esc[1], key(Scancode::J, true), key(Scancode::J, false)]);
    rig.frame(&esc);
    rig.frame(&[]);
    assert_eq!(rig.steps(2), [N, N]);
    assert_eq!(rig.controls.stats().presses, 0);

    // During the pause, or held from then until after the resume: never a press.
    let mut rig = Rig::new();
    rig.frame(&esc);
    rig.frame(&tap(Scancode::J));
    rig.frame(&[key(Scancode::K, true)]);
    rig.frame(&esc);
    rig.frame(&[]);
    assert_eq!(rig.steps(2), [N, N], "K is still down, but it was pressed during the pause");
    rig.frame(&[key(Scancode::K, false)]);
    rig.frame(&[key(Scancode::K, true)]);
    assert_eq!(rig.steps(2), [H, H], "pressed again, it counts");

    // Held through the pause: read released, then it needs a press, so it can't be doubled.
    let mut rig = Rig::new();
    rig.frame(&[key(Scancode::J, true)]);
    assert_eq!(rig.steps(1), [L]);
    rig.frame(&esc);
    rig.frame(&[]);
    rig.frame(&esc);
    rig.frame(&[]);
    assert_eq!(rig.steps(2), [N, N]);
    assert_eq!(rig.controls.stats().presses, 1);

    // The button that resumes is a verb too, and isn't one for this press.
    let mut rig = Rig::new();
    rig.frame(&[pad(PadButton::Start, true), pad(PadButton::Start, false)]);
    rig.frame(&[pad(PadButton::East, true)]);
    rig.frame(&[]);
    assert!(!rig.controls.is_paused());
    assert_eq!(rig.steps(2), [N, N], "East resumed, and it's still down");
    rig.frame(&[pad(PadButton::East, false)]);
    assert_eq!(rig.steps(1), [N]);
    rig.frame(&[pad(PadButton::East, true)]);
    assert_eq!(rig.steps(1), [H], "a new press of it is the heavy");

    // The key that opens the pause doesn't close it, however long it's held.
    let mut rig = Rig::new();
    rig.frame(&[esc[0]]);
    rig.frame(&[]);
    rig.frame(&[]);
    assert!(rig.controls.is_paused());
    rig.frame(&[esc[1]]);
    assert!(rig.controls.is_paused());
    rig.frame(&[esc[0], esc[1]]);
    rig.frame(&[]);
    assert!(!rig.controls.is_paused());

    // And the key that closes it doesn't open it again.
    rig.frame(&[esc[0]]);
    rig.frame(&[]);
    rig.frame(&[esc[1]]);
    rig.frame(&[]);
    assert!(rig.controls.is_paused(), "pressed afresh, it opens");
    rig.frame(&[esc[0]]);
    rig.frame(&[]);
    rig.frame(&[]);
    assert!(!rig.controls.is_paused(), "pressed afresh and held, it stays closed");
    rig.frame(&[]);
    assert!(!rig.controls.is_paused());
}

#[test]
fn a_stick_reads_its_level_from_the_first_step_after_the_pause() {
    let mut rig = Rig::new();
    rig.frame(&[axis(PadAxis::LeftX, 1.0)]);
    assert_eq!(rig.next().stick, Stick::new(127, 0));
    rig.frame(&tap(ESCAPE));
    rig.frame(&[]);
    rig.frame(&tap(ESCAPE));
    assert!(rig.controls.is_paused(), "the first step comes with the combat context");
    rig.frame(&[]);
    assert!(!rig.controls.is_paused());
    assert_eq!(rig.next().stick, Stick::new(127, 0));

    // Pushed during the pause, it reads as soon as there's a step too: a stick has no presses.
    let mut rig = Rig::new();
    rig.frame(&tap(ESCAPE));
    rig.frame(&[axis(PadAxis::LeftY, -1.0)]);
    rig.frame(&tap(ESCAPE));
    rig.frame(&[]);
    assert_eq!(rig.next().stick, Stick::new(0, 127));
}

#[test]
fn the_app_can_pause_too() {
    let mut rig = Rig::new();
    rig.controls.set_paused(true);
    assert!(rig.controls.is_paused(), "at once: the steps stop this frame");
    let mut driver = FixedStepDriver::default();
    assert_eq!(rig.controls.advance(&mut driver, Duration::from_millis(50)).count, 0);
    rig.frame(&tap(Scancode::J));
    assert_eq!(rig.controls.stats().presses, 0, "the combat context has gone");

    rig.controls.set_paused(false);
    assert!(rig.controls.is_paused(), "not until the combat context is back");
    rig.frame(&[]);
    assert!(!rig.controls.is_paused());

    // Changing its mind before the next update changes nothing.
    rig.controls.set_paused(true);
    rig.controls.set_paused(false);
    rig.controls.set_paused(true);
    rig.frame(&[]);
    assert!(rig.controls.is_paused());
    rig.controls.set_paused(false);
    rig.controls.set_paused(true);
    rig.frame(&[]);
    assert!(rig.controls.is_paused());
    rig.controls.set_paused(false);
    rig.frame(&[]);
    assert!(!rig.controls.is_paused());
    rig.frame(&tap(Scancode::J));
    assert_eq!(rig.steps(1), [L]);
}

/// A player and an opponent on the test moves, so a press starts something and a wrong tick shows.
fn fight() -> (Simulation, ActorId) {
    let scenario = find_scenario(SCENARIOS, "duel").expect("a scenario");
    let (sim, cast) = scenario.start(scenario.config).expect("sets up");
    (sim, cast[0])
}

/// Input for a run: at certain frames a tap of a verb or a stick moving.
fn script(frame: u32) -> Vec<RawInput> {
    match frame {
        3 => vec![key(Scancode::J, true), key(Scancode::J, false)],
        9 => vec![axis(PadAxis::LeftX, 0.7)],
        20 => vec![axis(PadAxis::LeftX, 0.0)],
        24 => vec![key(Scancode::E, true)],
        26 => vec![key(Scancode::E, false), key(Scancode::Q, true)],
        27 => vec![key(Scancode::Q, false)],
        33 => vec![key(Scancode::K, true), key(Scancode::K, false), key(Scancode::K, true)],
        35 => vec![key(Scancode::K, false)],
        40 => vec![pad(PadButton::South, true), pad(PadButton::South, false)],
        _ => vec![],
    }
}

/// The checksum after each tick, and each tick's input, of running [`script`], taking a pause and
/// mashing the keyboard through it wherever `pause_after` says.
fn run(pause_after: &[u32]) -> (Vec<u64>, Vec<TickActions>) {
    let (mut sim, player) = fight();
    let mut rig = Rig::on(CombatControls::with_default_bindings().expect("the default bindings"));
    let mut driver = FixedStepDriver::default();
    let frame_time = Duration::from_nanos(16_666_667);
    let (mut checksums, mut ticks) = (Vec::new(), Vec::new());
    let mut stepped = |rig: &mut Rig, driver: &mut FixedStepDriver, sim: &mut Simulation| {
        let steps = rig.controls.advance(driver, frame_time);
        for _ in 0..steps.count {
            let tick = rig.controls.next_tick(player, 0.0);
            sim.step(&tick);
            ticks.push(tick);
            checksums.push(sim.checksum());
        }
    };
    for frame in 0..60 {
        rig.frame(&script(frame));
        stepped(&mut rig, &mut driver, &mut sim);
        if pause_after.contains(&frame) {
            let (tick, checksum) = (sim.tick(), sim.checksum());
            rig.frame(&tap(ESCAPE));
            stepped(&mut rig, &mut driver, &mut sim);
            for pressed in [Scancode::J, Scancode::K, Scancode::E, Scancode::LEFT_SHIFT] {
                rig.frame(&tap(pressed));
                stepped(&mut rig, &mut driver, &mut sim);
                assert_eq!(sim.tick(), tick, "no tick while paused");
                assert_eq!(sim.checksum(), checksum, "no change while paused");
            }
            rig.frame(&tap(ESCAPE));
            stepped(&mut rig, &mut driver, &mut sim);
            assert_eq!(sim.tick(), tick, "not until the combat context is back");
            // The frame that brings it back steps, with the next frame of the script.
            assert!(rig.controls.is_paused());
        }
    }
    (checksums, ticks)
}

/// The simulation can't tell a pause happened: the same input gives the same bytes, tick for tick.
#[test]
fn checksums_are_unaffected_by_pausing() {
    let (checksums, ticks) = run(&[]);
    assert_eq!(checksums.len(), 60);
    assert!(ticks.iter().filter(|tick| !tick.is_empty()).count() > 10, "the script does something");

    // Nothing is held at these frames, so the pauses change nothing.
    for pause_after in [vec![0], vec![1, 2, 3], vec![22], vec![23, 40, 41], vec![45, 46, 47, 48, 59]] {
        let (paused_checksums, paused_ticks) = run(&pause_after);
        assert_eq!(paused_ticks, ticks, "pausing after {pause_after:?}: the same input");
        assert_eq!(paused_checksums, checksums, "pausing after {pause_after:?}: the same bytes");
    }
}

/// At Mobile-Low's two steps a frame, a tap is one press to the simulation, and a held press too.
#[test]
fn two_steps_a_frame_press_the_simulation_once() {
    let press = |sim: &mut Simulation, player, buttons| {
        let mut tick = TickActions::NEUTRAL;
        tick.set(player, ActorActions { buttons, stick: Stick::CENTER }).unwrap();
        sim.step(&tick);
    };

    let ((mut sim, player), (mut expected, _)) = (fight(), fight());
    let mut rig = Rig::new();
    rig.frame(&tap(Scancode::J));
    sim.step(&rig.controls.next_tick(player, 0.0));
    sim.step(&rig.controls.next_tick(player, 0.0));
    press(&mut expected, player, L);
    press(&mut expected, player, N);
    assert_eq!(sim.checksum(), expected.checksum(), "a tap");
    assert_eq!(sim.actor(player).expect("alive").moves.started, 1, "it started one move");

    let ((mut sim, player), (mut expected, _)) = (fight(), fight());
    let mut rig = Rig::new();
    rig.frame(&[key(Scancode::J, true)]);
    sim.step(&rig.controls.next_tick(player, 0.0));
    sim.step(&rig.controls.next_tick(player, 0.0));
    press(&mut expected, player, L);
    press(&mut expected, player, L);
    assert_eq!(sim.checksum(), expected.checksum(), "a held press");
    assert_eq!(sim.actor(player).expect("alive").moves.started, 1, "it started one move");
}
