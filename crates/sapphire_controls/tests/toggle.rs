//! Hold or toggle (M4 A4): a held action set to toggle reads, step by step, what holding its button from one
//! press to the next would have, so the simulation sees the same `TickActions` and no checksum moves.

mod support;

use sapphire_controls::{CombatControls, default_bindings, layered};
use sapphire_gameplay::{ActorId, TickActions};
use sapphire_input::{InputMap, PadButton, RawInput};
use sapphire_math::Pcg32;
use sapphire_platform::Scancode;
use support::{Rig, key, pad};

/// The actions a player may toggle, with the pad's button and the key that hold each.
const HELD: [(&str, PadButton, Scancode); 5] = [
    ("light", PadButton::West, Scancode::J),
    ("heavy", PadButton::East, Scancode::K),
    ("dodge", PadButton::South, Scancode::LEFT_SHIFT),
    ("skill", PadButton::North, Scancode::E),
    ("lock_on", PadButton::RightStick, Scancode::L),
];

const PLAYER: ActorId = ActorId::from_raw(1).expect("not zero");

/// One frame of a run: what goes in, a millisecond apart, and how many steps follow its update.
struct Frame {
    input: Vec<RawInput>,
    steps: usize,
}

fn ticks(controls: CombatControls, frames: &[Frame]) -> Vec<TickActions> {
    let mut rig = Rig::on(controls);
    let mut read = Vec::new();
    for frame in frames {
        rig.frame(&frame.input);
        read.extend((0..frame.steps).map(|_| rig.controls.next_tick(PLAYER, 0.0)));
    }
    read
}

/// A seeded run of holds of every held action, by pad and by key, as a list of frames twice over: held, the
/// button down for each hold, and toggled, a press as each hold starts and another as it ends.
fn runs(seed: u64) -> (Vec<Frame>, Vec<Frame>) {
    let mut random = Pcg32::new(seed, 0);
    let mut held = Vec::new();
    let mut toggled = Vec::new();
    // Each action's hold, and whether it's the pad's.
    let mut down: [Option<bool>; HELD.len()] = [None; HELD.len()];
    for _ in 0..240 {
        let steps = random.below(3) as usize;
        let (mut hold, mut toggle) = (Vec::new(), Vec::new());
        for (index, (_, button, scancode)) in HELD.iter().enumerate() {
            if random.below(6) != 0 {
                continue;
            }
            // A toggle's press may come from either device, since the latch is the action's; a hold lets
            // go of what it pressed.
            let by_pad = random.below(2) == 0;
            let press = |by_pad: bool, at: bool| if by_pad { pad(*button, at) } else { key(*scancode, at) };
            down[index] = match down[index] {
                Some(held_by_pad) => {
                    hold.push(press(held_by_pad, false));
                    None
                }
                None => {
                    hold.push(press(by_pad, true));
                    Some(by_pad)
                }
            };
            toggle.push(press(by_pad, true));
            toggle.push(press(by_pad, false));
        }
        held.push(Frame { input: hold, steps });
        toggled.push(Frame { input: toggle, steps });
    }
    (held, toggled)
}

#[test]
fn a_toggled_action_reads_as_the_same_hold_step_by_step() {
    let base = default_bindings().expect("the default bindings");
    let actions: Vec<&str> = HELD.iter().map(|(action, ..)| *action).collect();
    for seed in [1, 2, 3, 4] {
        let (held, toggled) = runs(seed);
        let holding = CombatControls::new(InputMap::new(base.clone()).unwrap()).unwrap();
        let toggling = CombatControls::new(InputMap::new(layered(&base, &[], &actions)).unwrap()).unwrap();
        let (by_hold, by_toggle) = (ticks(holding, &held), ticks(toggling, &toggled));
        assert!(by_hold.iter().any(|tick| *tick != TickActions::NEUTRAL), "seed {seed}: something was held");
        assert_eq!(by_hold.len(), by_toggle.len());
        for (step, (hold, toggle)) in by_hold.iter().zip(&by_toggle).enumerate() {
            assert_eq!(hold, toggle, "seed {seed}, step {step}");
        }
    }
}

/// Focus going lets a toggle go as it lets a held button go, so the step after reads them the same.
#[test]
fn losing_focus_lets_a_toggle_go_as_it_lets_a_hold_go() {
    let base = default_bindings().expect("the default bindings");
    let mut holding = Rig::on(CombatControls::new(InputMap::new(base.clone()).unwrap()).unwrap());
    let mut toggling =
        Rig::on(CombatControls::new(InputMap::new(layered(&base, &[], &["dodge"])).unwrap()).unwrap());
    holding.frame(&[key(Scancode::LEFT_SHIFT, true)]);
    toggling.frame(&[key(Scancode::LEFT_SHIFT, true), key(Scancode::LEFT_SHIFT, false)]);
    assert_eq!(holding.controls.next_tick(PLAYER, 0.0), toggling.controls.next_tick(PLAYER, 0.0));
    holding.focus_lost(1, &[Scancode::LEFT_SHIFT]);
    toggling.focus_lost(1, &[]);
    for rig in [&mut holding, &mut toggling] {
        rig.finish_frame();
    }
    let (hold, toggle) = (holding.controls.next_tick(PLAYER, 0.0), toggling.controls.next_tick(PLAYER, 0.0));
    assert_eq!(hold, toggle);
    assert_eq!(hold, TickActions::NEUTRAL, "both let go");
}
