//! What the mapping costs a frame, against spec §3.2's budget for platform events, input and
//! orchestration together (0.3 ms on Desktop-Low, 0.6 on Mobile-Low).
//!
//! ```text
//! cargo run --release -p sapphire_controls --example update_bench [frames]
//! ```
//!
//! Runs the default bindings for frames with no input, and frames with five inputs and a step, in
//! alternating rounds, and reports each kind's minimum median across rounds: `update` plus a
//! `next_tick` per step.

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

use std::time::Duration;

use sapphire_controls::CombatControls;
use sapphire_gameplay::ActorId;
use sapphire_input::{PadAxis, PadButton, RawInput, Timestamp};
use sapphire_platform::{GamepadType, Scancode};
use sapphire_time::Stopwatch;

const ROUNDS: usize = 9;

/// One frame's input on the frame's timeline.
fn busy_input(controls: &mut CombatControls, frame: u64, now: u64) {
    let down = frame.is_multiple_of(2);
    let stick = (frame as f32 * 0.1).sin();
    let inputs = [
        RawInput::PadAxis { pad: 0, axis: PadAxis::LeftX, value: stick },
        RawInput::PadAxis { pad: 0, axis: PadAxis::RightX, value: stick },
        RawInput::PadButton { pad: 0, button: PadButton::West, down },
        RawInput::Key { scancode: Scancode::K, down },
        RawInput::Key { scancode: Scancode::W, down: !down },
    ];
    for (offset, input) in inputs.into_iter().enumerate() {
        controls.input_mut().push(Timestamp::from_nanos(now + offset as u64 + 1), input);
    }
}

/// The median frame of `frames`.
fn round(controls: &mut CombatControls, frames: u64, clock: &mut u64, busy: bool) -> Duration {
    let player = ActorId::from_raw(1).expect("not zero");
    let mut lengths: Vec<Duration> = Vec::with_capacity(frames as usize);
    for frame in 0..frames {
        *clock += 16_666_667;
        if busy {
            busy_input(controls, frame, *clock);
        }
        let stopwatch = Stopwatch::start();
        controls.update(Timestamp::from_nanos(*clock + 1_000));
        if busy {
            std::hint::black_box(controls.next_tick(player, 0.7));
        }
        lengths.push(stopwatch.elapsed());
    }
    lengths.sort_unstable();
    lengths[lengths.len() / 2]
}

fn main() {
    let frames = std::env::args().nth(1).and_then(|raw| raw.parse().ok()).unwrap_or(20_000);
    let mut controls = CombatControls::with_default_bindings().expect("the default bindings");
    controls
        .input_mut()
        .push(Timestamp::default(), RawInput::GamepadAdded { pad: 0, kind: GamepadType::XboxOne });
    let mut clock = 0;
    let (mut idle, mut busy) = (Duration::MAX, Duration::MAX);
    for _ in 0..ROUNDS {
        idle = idle.min(round(&mut controls, frames, &mut clock, false));
        busy = busy.min(round(&mut controls, frames, &mut clock, true));
    }
    let bindings: usize =
        controls.input().bindings().contexts.iter().map(|context| context.bindings.len()).sum();
    println!("{bindings} bindings, {frames} frames a round, {ROUNDS} rounds, minimum median:");
    println!("  no input: {:.2} µs", idle.as_secs_f64() * 1e6);
    println!("  five inputs and a step: {:.2} µs", busy.as_secs_f64() * 1e6);
    println!("  stats: {:?}", controls.stats());
}
