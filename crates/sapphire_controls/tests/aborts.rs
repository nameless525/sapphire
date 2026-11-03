//! What each of `sapphire_input`'s `Abort` rules does to a tick: a player's release (`No`), a device
//! letting go (`Release`: a pad disconnected) and everything letting go (`All`: focus or the
//! foreground lost), for verbs held, tapped and armed.

mod support;

use sapphire_controls::{CombatControls, default_bindings};
use sapphire_gameplay::{Buttons, Stick};
use sapphire_input::{InputMap, PadAxis, PadButton, RawInput, Trigger};
use sapphire_platform::{GamepadType, Scancode};
use support::{H, L, N, Rig, axis, key, pad};

const J: Scancode = Scancode::J;
const K: Scancode = Scancode::K;
const REMOVED: RawInput = RawInput::GamepadRemoved { pad: 0 };

/// The default bindings with these named bindings' triggers replaced.
fn rebound(triggers: &[(&str, Trigger)]) -> Rig {
    let mut document = default_bindings().expect("the default bindings");
    for (name, trigger) in triggers {
        let binding = document
            .contexts
            .iter_mut()
            .flat_map(|context| &mut context.bindings)
            .find(|binding| binding.name.as_deref() == Some(*name))
            .expect("a named binding");
        binding.trigger = *trigger;
    }
    Rig::on(CombatControls::new(InputMap::new(document).expect("compiles")).expect("combat's actions"))
}

const TAP: Trigger = Trigger::Tap { seconds: 0.2 };
const DOUBLE_TAP: Trigger = Trigger::DoubleTap { seconds: 0.5 };

#[test]
fn abort_no_a_player_letting_go() {
    let mut rig = Rig::new();
    rig.frame(&[key(J, true), axis(PadAxis::LeftX, 1.0)]);
    assert_eq!(rig.steps(1), [L]);
    rig.frame(&[key(J, false), axis(PadAxis::LeftX, 0.0)]);
    let next = rig.next();
    assert_eq!((next.buttons, next.stick), (N, Stick::CENTER), "reads released, and no press comes of it");

    // A tap binding fires on its release, and is one press.
    let mut rig = rebound(&[("light.keyboard", TAP)]);
    rig.frame(&[key(J, true)]);
    assert_eq!(rig.steps(1), [N], "held, no press yet");
    rig.frame(&[key(J, false)]);
    assert_eq!(rig.steps(2), [L, N]);
}

#[test]
fn abort_release_a_pad_disconnecting_lets_go_of_that_pad_alone() {
    // Held on the pad and on the keyboard.
    let mut rig = Rig::new();
    rig.frame(&[pad(PadButton::West, true), key(K, true), axis(PadAxis::LeftX, 1.0)]);
    let next = rig.next();
    assert_eq!((next.buttons, next.stick), (L | H, Stick::new(127, 0)));
    rig.frame(&[REMOVED]);
    let next = rig.next();
    assert_eq!(
        (next.buttons, next.stick),
        (H, Stick::CENTER),
        "the pad's button and stick let go, and the key stays held"
    );
    assert_eq!(rig.steps(1), [H], "no press in that");

    // A press made before the disconnect still arrives.
    let mut rig = Rig::new();
    rig.frame(&[pad(PadButton::West, true), REMOVED]);
    assert_eq!(rig.steps(2), [L, N]);

    // A tap in progress on the pad fires nothing.
    let mut rig = rebound(&[("light.gamepad", TAP)]);
    rig.frame(&[pad(PadButton::West, true)]);
    rig.frame(&[REMOVED]);
    assert_eq!(rig.steps(2), [N, N]);

    // A double tap armed on the keyboard stays armed when the pad goes.
    let mut rig = rebound(&[("heavy.keyboard", DOUBLE_TAP)]);
    rig.frame(&[key(K, true), key(K, false)]);
    assert_eq!(rig.steps(1), [N], "armed");
    rig.frame(&[REMOVED]);
    rig.frame(&[key(K, true)]);
    assert_eq!(rig.steps(2), [H, N], "the second press fires");

    // A pad that comes back reads nothing until it's used, and its next press counts once.
    let mut rig = Rig::new();
    rig.frame(&[pad(PadButton::West, true), axis(PadAxis::LeftY, -1.0)]);
    assert_eq!(rig.steps(1), [L]);
    rig.frame(&[REMOVED]);
    assert_eq!(rig.steps(1), [N]);
    rig.frame(&[RawInput::GamepadAdded { pad: 0, kind: GamepadType::XboxOne }]);
    let next = rig.next();
    assert_eq!((next.buttons, next.stick), (N, Stick::CENTER));
    rig.frame(&[pad(PadButton::West, true)]);
    assert_eq!(rig.steps(2), [L, L]);
}

#[test]
fn abort_all_losing_focus_or_the_foreground_lets_go_of_everything() {
    enum Loss {
        Focus,
        Background,
    }
    for (name, loss) in [("focus", Loss::Focus), ("the foreground", Loss::Background)] {
        let lose = |rig: &mut Rig, held: &[Scancode]| match loss {
            Loss::Focus => rig.focus_lost(1, held),
            // SDL also posts the key-ups going to the background.
            Loss::Background => {
                for &code in held {
                    rig.platform_key(1, code, false);
                }
                rig.backgrounded(1);
            }
        };

        // Keyboard and pad alike read released at the next step, and that's no press.
        let mut rig = Rig::new();
        rig.frame(&[key(J, true), pad(PadButton::East, true), axis(PadAxis::LeftX, 1.0)]);
        let next = rig.next();
        assert_eq!((next.buttons, next.stick), (L | H, Stick::new(127, 0)), "{name}");
        lose(&mut rig, &[J]);
        rig.finish_frame();
        let next = rig.next();
        assert_eq!((next.buttons, next.stick), (N, Stick::CENTER), "{name}");

        // A press made before the loss, in the same frame, still arrives once. Its key-up, which SDL
        // made up, isn't taken for a release, so the binding judges no tap from it.
        let mut rig = Rig::new();
        rig.platform_key(1, J, true);
        lose(&mut rig, &[J]);
        rig.finish_frame();
        assert_eq!(rig.steps(2), [L, N], "{name}");

        // A tap in progress fires nothing, though SDL's key-up looks like its release.
        let mut rig = rebound(&[("light.keyboard", TAP)]);
        rig.frame(&[key(J, true)]);
        lose(&mut rig, &[J]);
        rig.finish_frame();
        assert_eq!(rig.steps(2), [N, N], "{name}");

        // A double tap goes idle, so the first press after the return starts a new one.
        let mut rig = rebound(&[("heavy.keyboard", DOUBLE_TAP)]);
        rig.frame(&[key(K, true), key(K, false)]);
        assert_eq!(rig.steps(1), [N], "{name}: armed");
        lose(&mut rig, &[]);
        rig.finish_frame();
        rig.frame(&[key(K, true)]);
        assert_eq!(rig.steps(2), [N, N], "{name}: disarmed");

        // The real release after the loss finds nothing, and the next press counts once.
        let mut rig = Rig::new();
        rig.frame(&[key(J, true)]);
        assert_eq!(rig.steps(1), [L], "{name}");
        lose(&mut rig, &[J]);
        rig.finish_frame();
        assert_eq!(rig.steps(1), [N], "{name}");
        rig.frame(&[key(J, false)]);
        assert_eq!(rig.steps(1), [N], "{name}");
        rig.frame(&[key(J, true)]);
        assert_eq!(rig.steps(2), [L, L], "{name}");
    }
}

/// The ultimate is the right trigger's, the one verb on an axis. An axis reports only as it moves, so
/// a trigger held through a loss says so with its first jitter on the way back.
#[test]
fn a_trigger_held_through_a_loss_is_no_new_press() {
    const ULTIMATE: Buttons = Buttons::ULTIMATE;
    let mut rig = Rig::new();
    rig.frame(&[axis(PadAxis::RightTrigger, 1.0)]);
    assert_eq!(rig.steps(1), [ULTIMATE]);
    assert_eq!(rig.controls.stats().presses, 1);
    rig.focus_lost(1, &[]);
    rig.finish_frame();
    assert_eq!(rig.steps(1), [N]);

    rig.frame(&[axis(PadAxis::RightTrigger, 0.98)]);
    assert_eq!(rig.steps(2), [N, N], "it was down all along");
    assert_eq!(rig.controls.stats().presses, 1);

    rig.frame(&[axis(PadAxis::RightTrigger, 0.0)]);
    rig.frame(&[axis(PadAxis::RightTrigger, 1.0)]);
    assert_eq!(rig.steps(1), [ULTIMATE], "let go and pressed, it counts");
    assert_eq!(rig.controls.stats().presses, 2);
}

#[test]
fn on_a_loss_the_stats_count_no_press_that_was_not_made() {
    let mut rig = Rig::new();
    rig.frame(&[key(J, true), key(K, true)]);
    assert_eq!(rig.controls.stats().presses, 2);
    rig.focus_lost(1, &[J, K]);
    rig.finish_frame();
    assert_eq!(rig.controls.stats().presses, 2);
    assert_eq!(rig.steps(2), [L | H, Buttons::NONE], "the two presses made, and then nothing");
}
