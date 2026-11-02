//! The part that matters: no press is lost or doubled, however frames and steps line up.
//!
//! The app judges input per frame and steps 0, 1 or 2 times a frame, so each row is a list of frames:
//! the input in it, and what each of its steps must read. The simulation finds a press by comparing
//! one tick with the last, so each row also says how many presses that comparison must find.

mod support;

use sapphire_gameplay::Buttons;
use sapphire_input::{MouseButton, PadAxis, PadButton, RawInput};
use sapphire_math::Pcg32;
use sapphire_platform::Scancode;
use support::{D, Edges, L, N, Rig, axis, control, key, mouse, pad};

struct Frame {
    input: Vec<RawInput>,
    steps: Vec<Buttons>,
}

fn f(input: &[RawInput], steps: &[Buttons]) -> Frame {
    Frame { input: input.to_vec(), steps: steps.to_vec() }
}

struct Row {
    name: &'static str,
    presses: u32,
    frames: Vec<Frame>,
}

/// Where a row goes wrong, if it does.
fn run(row: &Row) -> Result<(), String> {
    let mut rig = Rig::new();
    let mut edges = Edges::default();
    for (index, frame) in row.frames.iter().enumerate() {
        rig.frame(&frame.input);
        let read = rig.steps(frame.steps.len());
        if read != frame.steps {
            return Err(format!("frame {index}: steps read {read:?}, expected {:?}", frame.steps));
        }
        read.into_iter().for_each(|buttons| edges.see(buttons));
    }
    if edges.total() != row.presses {
        return Err(format!("the simulation saw {} presses, expected {}", edges.total(), row.presses));
    }
    let stats = rig.controls.stats();
    if stats.presses_over_limit != 0 {
        return Err(format!("{} presses were over the limit", stats.presses_over_limit));
    }
    Ok(())
}

/// What must hold for one button, on any device: `p` presses it and `r` lets go.
fn rows(p: RawInput, r: RawInput) -> Vec<Row> {
    let row = |name, presses, frames| Row { name, presses, frames };
    vec![
        row("a tap inside one frame reaches one step", 1, vec![f(&[p, r], &[L]), f(&[], &[N]), f(&[], &[N])]),
        row(
            "a tap between steps, over frames with none, reaches one step",
            1,
            vec![f(&[p], &[]), f(&[r], &[]), f(&[], &[L]), f(&[], &[N])],
        ),
        row("a tap at two steps a frame reads down, then up", 1, vec![f(&[p, r], &[L, N]), f(&[], &[N, N])]),
        row(
            "a press held at two steps a frame is one press",
            1,
            vec![f(&[p], &[L, L]), f(&[], &[L, L]), f(&[r], &[N, N]), f(&[], &[N, N])],
        ),
        row(
            "a press in a frame with no step waits for the next",
            1,
            vec![f(&[p], &[]), f(&[], &[L]), f(&[r], &[N])],
        ),
        row(
            "a release in a frame with no step waits too",
            1,
            vec![f(&[p], &[L]), f(&[r], &[]), f(&[], &[N])],
        ),
        row(
            "a held press through frames with and without steps stays one press",
            1,
            vec![f(&[p], &[L]), f(&[], &[]), f(&[], &[L, L]), f(&[], &[]), f(&[], &[L]), f(&[r], &[N])],
        ),
        row("two taps in one frame are two presses", 2, vec![f(&[p, r, p, r], &[L, N, L, N]), f(&[], &[N])]),
        row(
            "two taps at two steps a frame",
            2,
            vec![f(&[p, r, p, r], &[L, N]), f(&[], &[L, N]), f(&[], &[N, N])],
        ),
        row("a tap, then a hold, in one frame", 2, vec![f(&[p, r, p], &[L, N, L, L]), f(&[r], &[N])]),
        row(
            "let go and pressed again between two steps",
            2,
            vec![f(&[p], &[L]), f(&[r, p], &[N]), f(&[], &[L]), f(&[r], &[N])],
        ),
        row(
            "let go and pressed again, then let go, between two steps",
            2,
            vec![f(&[p], &[L]), f(&[r, p, r], &[N]), f(&[], &[L]), f(&[], &[N])],
        ),
        row(
            "taps in frames with no step all arrive",
            3,
            vec![
                f(&[p, r], &[]),
                f(&[p, r], &[]),
                f(&[p, r], &[L, N]),
                f(&[], &[L, N]),
                f(&[], &[L, N]),
                f(&[], &[N]),
            ],
        ),
        row(
            "as many taps as can wait, all arrive",
            4,
            vec![f(&[p, r, p, r, p, r, p, r], &[L, N, L, N, L, N, L, N]), f(&[], &[N])],
        ),
        row("nothing pressed reads nothing", 0, vec![f(&[], &[N, N]), f(&[], &[]), f(&[], &[N])]),
    ]
}

#[test]
fn every_row_holds_on_every_device() {
    let on_screen = {
        let rig = Rig::new();
        rig.controls.input().map().virtual_control("light").expect("an on-screen light")
    };
    let devices: [(&str, RawInput, RawInput); 4] = [
        ("keyboard", key(Scancode::J, true), key(Scancode::J, false)),
        ("mouse", mouse(MouseButton::Left, true), mouse(MouseButton::Left, false)),
        ("gamepad", pad(PadButton::West, true), pad(PadButton::West, false)),
        ("on screen", control(on_screen, true), control(on_screen, false)),
    ];
    let mut failures = Vec::new();
    for (device, press, release) in devices {
        for row in rows(press, release) {
            if let Err(error) = run(&row) {
                failures.push(format!("{device}, {}: {error}", row.name));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn buttons_are_independent() {
    let (light, light_up) = (key(Scancode::J, true), key(Scancode::J, false));
    let (dodge, dodge_up) = (pad(PadButton::South, true), pad(PadButton::South, false));
    let both = L | D;
    let table = [
        Row {
            name: "pressed in one frame, one let go",
            presses: 2,
            frames: vec![f(&[light, dodge, light_up], &[both, D]), f(&[dodge_up], &[N])],
        },
        Row {
            name: "one tapping while the other holds",
            presses: 3,
            frames: vec![
                f(&[dodge], &[D]),
                f(&[light, light_up], &[both, D]),
                f(&[light, light_up], &[both]),
                f(&[dodge_up], &[N]),
            ],
        },
    ];
    let failures: Vec<_> =
        table.iter().filter_map(|row| run(row).err().map(|error| format!("{}: {error}", row.name))).collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn two_devices_on_one_action_are_one_press() {
    let (key_down, key_up) = (key(Scancode::J, true), key(Scancode::J, false));
    let (pad_down, pad_up) = (pad(PadButton::West, true), pad(PadButton::West, false));
    run(&Row {
        name: "pad, then key, then pad up, then key up",
        presses: 1,
        frames: vec![f(&[pad_down], &[L]), f(&[key_down], &[L]), f(&[pad_up], &[L]), f(&[key_up], &[N])],
    })
    .unwrap();
}

#[test]
fn an_analog_trigger_presses_at_its_threshold() {
    let trigger = |value| axis(PadAxis::RightTrigger, value);
    const U: Buttons = Buttons::ULTIMATE;
    run(&Row {
        name: "the ultimate on the right trigger, half a pull",
        presses: 2,
        frames: vec![
            f(&[trigger(0.4)], &[N]),
            f(&[trigger(0.6)], &[U]),
            f(&[trigger(1.0)], &[U]),
            f(&[trigger(0.4)], &[N]),
            f(&[trigger(0.5)], &[U]),
            f(&[trigger(0.0)], &[N]),
        ],
    })
    .unwrap();
}

#[test]
fn presses_past_the_limit_are_counted() {
    let mut rig = Rig::new();
    let (p, r) = (key(Scancode::J, true), key(Scancode::J, false));
    rig.frame(&[p, r, p, r, p, r, p, r, p, r, p, r]);
    let stats = rig.controls.stats();
    assert_eq!((stats.presses, stats.presses_over_limit), (6, 2));
    assert_eq!(rig.steps(10), [L, N, L, N, L, N, L, N, N, N], "the four that could wait");
}

/// Whatever frames do: every press the mapping took is one edge, for every button, and nothing is
/// left held once everything's let go.
#[test]
fn random_frames_lose_and_double_nothing() {
    let keys = [
        (Scancode::J, Buttons::LIGHT),
        (Scancode::K, Buttons::HEAVY),
        (Scancode::LEFT_SHIFT, Buttons::DODGE),
    ];
    for seed in 0..200 {
        let mut rng = Pcg32::new(seed, 1);
        let mut rig = Rig::new();
        let mut edges = Edges::default();
        let mut down = [false; 3];
        let mut pressed = [0u32; 3];
        for _ in 0..200 {
            let mut inputs = Vec::new();
            for (index, (scancode, _)) in keys.iter().enumerate() {
                // Some frames change nothing, and some change a button several times.
                for _ in 0..rng.below(4).saturating_sub(1) {
                    down[index] = !down[index];
                    pressed[index] += u32::from(down[index]);
                    inputs.push(key(*scancode, down[index]));
                }
            }
            rig.frame(&inputs);
            // Steps as frames at 120, 60 and 30 Hz would run them.
            for buttons in rig.steps(rng.below(3) as usize + (inputs.len() > 3) as usize) {
                edges.see(buttons);
            }
        }
        // Everything lets go, and the steps catch up.
        let up: Vec<_> = keys.iter().map(|(scancode, _)| key(*scancode, false)).collect();
        rig.frame(&up);
        let drained = rig.steps(32);
        drained.iter().for_each(|&buttons| edges.see(buttons));
        assert_eq!(drained[31], N, "seed {seed}: nothing stays held");

        let stats = rig.controls.stats();
        assert_eq!(stats.presses, u64::from(pressed.iter().sum::<u32>()), "seed {seed}: every press counted");
        assert_eq!(
            stats.presses - stats.presses_over_limit,
            u64::from(edges.total()),
            "seed {seed}: one edge for each, as {pressed:?} pressed and {:?} seen",
            edges.by_bit
        );
    }
}

#[test]
fn a_rebound_button_presses_as_before() {
    let mut rig = Rig::new();
    let (j_down, j_up) = (key(Scancode::J, true), key(Scancode::J, false));
    rig.frame(&[j_down, j_up]);
    assert_eq!(rig.steps(2), [L, N]);

    let previous = rig
        .controls
        .input_mut()
        .rebind("light.keyboard", sapphire_input::Source::Key(sapphire_input::Key::U))
        .expect("the binding is named");
    assert_eq!(previous, sapphire_input::Source::Key(sapphire_input::Key::J));
    rig.frame(&[]);

    rig.frame(&[j_down, j_up]);
    assert_eq!(rig.steps(1), [N], "the old key does nothing");
    rig.frame(&[key(Scancode::U, true), key(Scancode::U, false)]);
    assert_eq!(rig.steps(2), [L, N], "the new key does");

    // A key held through the rebinding isn't a press of the new binding.
    rig.frame(&[key(Scancode::K, true)]);
    assert_eq!(rig.steps(1), [support::H]);
    rig.controls.input_mut().rebind("heavy.gamepad", sapphire_input::Source::Pad(PadButton::DpadUp)).unwrap();
    rig.frame(&[]);
    assert_eq!(rig.steps(1), [N], "every binding starts afresh, so what's held reads released");
    rig.frame(&[key(Scancode::K, false)]);
    rig.frame(&[key(Scancode::K, true)]);
    assert_eq!(rig.steps(1), [support::H]);
}
