//! The whole pipeline, driven with synthetic raw input and timestamps: triggers, chords,
//! context priority and consumption, modifiers, gestures, on-screen controls, rebinding, and
//! the timestamps every action event carries.

use sapphire_input::ActionEventKind::{Canceled, Completed, Started, Triggered};
use sapphire_input::{
    ActionEventKind, ActionId, DeviceKind, GamepadFamily, InputMap, InputSystem, Key, PadAxis, PadButton,
    RawInput, RebindError, Source, Timestamp,
};
use sapphire_math::Vec2;
use sapphire_platform::{Event, GamepadType, LifecycleEvent, Scancode, TouchPhase};

fn ms(ms: u64) -> Timestamp {
    Timestamp::from_nanos(ms * 1_000_000)
}

/// A map from the inside of a bindings document's `actions` and `contexts` lists.
fn map(actions: &str, contexts: &str) -> InputMap {
    let text = format!(
        r#"(type: "sapphire::input::InputBindings", version: 1, data: (actions: [{actions}], contexts: [{contexts}]))"#
    );
    InputMap::from_ron(&text).unwrap_or_else(|error| panic!("{error}"))
}

type Logged = (String, ActionEventKind, u64);

fn ev(action: &str, kind: ActionEventKind, ms: u64) -> Logged {
    (action.to_owned(), kind, ms)
}

struct Rig {
    input: InputSystem,
}

impl Rig {
    /// A system with `contexts` enabled at priority 0, in order, and a first update at 0 ms.
    fn new(map: InputMap, contexts: &[&str]) -> Self {
        let mut input = InputSystem::new(map);
        for name in contexts {
            let context = input.map().context(name).expect("context");
            input.enable_context(context, 0);
        }
        input.update(ms(0));
        Self { input }
    }

    fn action(&self, name: &str) -> ActionId {
        self.input.map().action(name).expect("action")
    }

    fn push(&mut self, t: u64, input: RawInput) {
        self.input.push(ms(t), input);
    }

    fn key(&mut self, t: u64, scancode: Scancode, down: bool) {
        self.push(t, RawInput::Key { scancode, down });
    }

    fn touch(&mut self, t: u64, finger: usize, phase: TouchPhase, x: f32, y: f32) {
        self.push(t, RawInput::Touch { finger, phase, position: Vec2::new(x, y) });
    }

    /// Updates at `t` and returns the events as (action, kind, ms), checking they are in order.
    fn update(&mut self, t: u64) -> Vec<Logged> {
        self.input.update(ms(t));
        let events = self.input.events();
        assert!(events.windows(2).all(|pair| pair[0].timestamp <= pair[1].timestamp), "events in time order");
        events
            .iter()
            .map(|e| {
                (
                    self.input.map().action_name(e.action).to_owned(),
                    e.kind,
                    e.timestamp.as_nanos() / 1_000_000,
                )
            })
            .collect()
    }

    /// The events of one action only.
    fn update_for(&mut self, t: u64, action: &str) -> Vec<Logged> {
        self.update(t).into_iter().filter(|(name, ..)| name == action).collect()
    }

    fn value(&self, action: &str) -> Vec2 {
        self.input.state(self.action(action)).value()
    }
}

#[test]
fn events_carry_their_device_timestamps_and_a_quick_press_counts() {
    let map = map(
        r#"(name: "jump", kind: Button), (name: "walk", kind: Button)"#,
        r#"(name: "main", bindings: [
            (action: "jump", source: Key(Space), trigger: Pressed),
            (action: "walk", source: Key(W)),
        ])"#,
    );
    let mut rig = Rig::new(map, &["main"]);
    // Everything happens between two frames.
    rig.key(12, Scancode::W, true);
    rig.key(20, Scancode::SPACE, true);
    rig.key(25, Scancode::SPACE, false);
    rig.key(30, Scancode::W, false);
    assert_eq!(
        rig.update(33),
        [
            ev("walk", Started, 12),
            ev("walk", Triggered, 12),
            ev("jump", Started, 20),
            ev("jump", Triggered, 20),
            ev("jump", Completed, 20),
            ev("walk", Completed, 30),
        ]
    );
    let jump = rig.input.state(rig.action("jump"));
    assert!(jump.fired() && !jump.is_triggered(), "pressed this frame, released by its end");
    assert_eq!(rig.update(50), [], "events last one update");
    assert!(!rig.input.state(rig.action("jump")).fired());
}

#[test]
fn timestamps_hand_off_to_the_fixed_step() {
    // A frame loop: pump, update with the frame's time, then hand the triggered events to the
    // frame's first fixed step, which buffers them as intents and judges their age itself.
    let map = map(
        r#"(name: "attack", kind: Button)"#,
        r#"(name: "main", bindings: [(action: "attack", source: Key(J), trigger: Pressed)])"#,
    );
    let mut rig = Rig::new(map, &["main"]);
    let attack = rig.action("attack");
    let mut intents: Vec<Timestamp> = Vec::new();
    let mut frame = |rig: &mut Rig, now: u64| {
        rig.input.update(ms(now));
        intents.extend(
            rig.input
                .events()
                .iter()
                .filter(|e| e.action == attack && e.kind == Triggered)
                .map(|e| e.timestamp),
        );
    };

    rig.key(3, Scancode::J, true);
    rig.key(9, Scancode::J, false);
    rig.key(11, Scancode::J, true);
    // A hitch: the frame runs 80 ms after the presses.
    frame(&mut rig, 91);
    assert_eq!(intents, [ms(3), ms(11)], "both presses, at their own times");
    // So a 100 ms input buffer can still honor the first press, and knows it is 88 ms old.
    let age = ms(91).saturating_duration_since(intents[0]);
    assert_eq!(age.as_millis(), 88);
}

#[test]
fn a_hold_triggers_at_its_press_plus_the_hold_time() {
    let map = map(
        r#"(name: "charge", kind: Button)"#,
        r#"(name: "main", bindings: [(action: "charge", source: Key(J), trigger: Hold(seconds: 0.5))])"#,
    );
    let mut rig = Rig::new(map, &["main"]);
    rig.key(100, Scancode::J, true);
    assert_eq!(rig.update(116), [ev("charge", Started, 100)]);
    // The frame runs late; the event still says when the hold completed.
    assert_eq!(rig.update(650), [ev("charge", Triggered, 600)]);
    assert!(rig.input.state(rig.action("charge")).is_triggered());
    assert_eq!(rig.input.state(rig.action("charge")).since(), ms(600));
    rig.key(700, Scancode::J, false);
    assert_eq!(rig.update(716), [ev("charge", Completed, 700)]);

    // Let go early: canceled.
    rig.key(1000, Scancode::J, true);
    rig.key(1200, Scancode::J, false);
    assert_eq!(rig.update(1216), [ev("charge", Started, 1000), ev("charge", Canceled, 1200)]);
}

#[test]
fn taps_holds_and_double_taps_share_a_key() {
    let map = map(
        r#"(name: "light", kind: Button), (name: "heavy", kind: Button), (name: "dash", kind: Button)"#,
        r#"(name: "main", bindings: [
            (action: "light", source: Key(J), trigger: Tap(seconds: 0.2)),
            (action: "heavy", source: Key(J), trigger: Hold(seconds: 0.5)),
            (action: "dash", source: Key(K), trigger: DoubleTap(seconds: 0.3)),
        ])"#,
    );
    let mut rig = Rig::new(map, &["main"]);
    // A tap: light fires on release, heavy cancels.
    rig.key(100, Scancode::J, true);
    rig.key(250, Scancode::J, false);
    assert_eq!(
        rig.update(266),
        [
            ev("light", Started, 100),
            ev("heavy", Started, 100),
            ev("light", Triggered, 250),
            ev("light", Completed, 250),
            ev("heavy", Canceled, 250),
        ]
    );

    // A hold: the tap runs out at 200 ms, the hold completes at 500 ms.
    rig.key(1000, Scancode::J, true);
    assert_eq!(
        rig.update(1400),
        [ev("light", Started, 1000), ev("heavy", Started, 1000), ev("light", Canceled, 1200)]
    );
    assert_eq!(rig.update(1600), [ev("heavy", Triggered, 1500)]);
    rig.key(1700, Scancode::J, false);
    assert_eq!(rig.update(1716), [ev("heavy", Completed, 1700)]);

    // A double tap fires on the second press.
    rig.key(2000, Scancode::K, true);
    rig.key(2080, Scancode::K, false);
    rig.key(2200, Scancode::K, true);
    rig.key(2250, Scancode::K, false);
    assert_eq!(
        rig.update(2266),
        [ev("dash", Started, 2000), ev("dash", Triggered, 2200), ev("dash", Completed, 2200)]
    );
    // Too slow: the window closes at 300 ms after the first release.
    rig.key(3000, Scancode::K, true);
    rig.key(3050, Scancode::K, false);
    assert_eq!(rig.update(3400), [ev("dash", Started, 3000), ev("dash", Canceled, 3350)]);
}

#[test]
fn chords_gate_their_binding_and_block_the_plain_one() {
    let map = map(
        r#"(name: "save", kind: Button), (name: "back", kind: Button), (name: "ctrl", kind: Button)"#,
        r#"(name: "main", bindings: [
            (action: "save", source: Key(S), trigger: Pressed, chord: "ctrl"),
            (action: "back", source: Key(S), trigger: Pressed),
            (action: "ctrl", source: Key(LeftCtrl)),
        ])"#,
    );
    let mut rig = Rig::new(map, &["main"]);
    let fired = |events: Vec<Logged>| -> Vec<String> {
        events.into_iter().filter(|(_, kind, _)| *kind == Triggered).map(|(name, ..)| name).collect()
    };

    rig.key(10, Scancode::S, true);
    rig.key(20, Scancode::S, false);
    assert_eq!(fired(rig.update(30)), ["back"]);

    rig.key(100, Scancode::LEFT_CTRL, true);
    rig.key(110, Scancode::S, true);
    rig.key(120, Scancode::S, false);
    rig.key(130, Scancode::LEFT_CTRL, false);
    assert_eq!(fired(rig.update(140)), ["ctrl", "save"], "Ctrl+S saves and doesn't go back");

    // S first, then Ctrl: S's press came before the chord, so nothing saves.
    rig.key(200, Scancode::S, true);
    rig.key(210, Scancode::LEFT_CTRL, true);
    assert_eq!(fired(rig.update(220)), ["back", "ctrl"]);
}

#[test]
fn higher_contexts_consume_their_sources() {
    let map = map(
        r#"(name: "confirm", kind: Button), (name: "look", kind: Button, consume: false),
           (name: "jump", kind: Button), (name: "peek", kind: Button)"#,
        r#"(name: "menu", bindings: [
               (action: "confirm", source: Key(Space), trigger: Pressed),
               (action: "look", source: Key(L), trigger: Pressed),
           ]),
           (name: "explore", bindings: [
               (action: "jump", source: Key(Space), trigger: Pressed),
               (action: "peek", source: Key(L), trigger: Pressed),
           ])"#,
    );
    let mut input = InputSystem::new(map);
    let (menu, explore) =
        (input.map().context("menu").expect("menu"), input.map().context("explore").expect("explore"));
    input.enable_context(explore, 0);
    input.enable_context(menu, 10);
    let mut rig = Rig { input };
    let tap = |rig: &mut Rig, t: u64, key: Scancode| -> Vec<String> {
        rig.key(t, key, true);
        rig.key(t + 5, key, false);
        let mut fired: Vec<_> = rig
            .update(t + 10)
            .into_iter()
            .filter(|(_, kind, _)| *kind == Triggered)
            .map(|(name, ..)| name)
            .collect();
        fired.sort();
        fired
    };

    assert_eq!(tap(&mut rig, 10, Scancode::SPACE), ["confirm"], "the menu consumes Space");
    assert_eq!(
        tap(&mut rig, 30, Scancode::L),
        ["look", "peek"],
        "a non-consuming action passes its source on"
    );

    rig.input.disable_context(menu);
    assert_eq!(tap(&mut rig, 50, Scancode::SPACE), ["jump"]);

    // Equal priorities stack: the most recently enabled wins.
    rig.input.enable_context(menu, 0);
    assert_eq!(tap(&mut rig, 70, Scancode::SPACE), ["confirm"]);
    rig.input.enable_context(explore, 0);
    assert_eq!(tap(&mut rig, 90, Scancode::SPACE), ["jump"]);
}

#[test]
fn context_switches_ignore_held_presses_and_complete_what_they_remove() {
    let map = map(
        r#"(name: "pause", kind: Button), (name: "close", kind: Button), (name: "walk", kind: Button)"#,
        r#"(name: "game", bindings: [
               (action: "pause", source: Key(Escape), trigger: Pressed),
               (action: "walk", source: Key(W)),
           ]),
           (name: "menu", bindings: [(action: "close", source: Key(Escape), trigger: Pressed)])"#,
    );
    let mut rig = Rig::new(map, &["game"]);
    rig.key(10, Scancode::W, true);
    rig.key(20, Scancode::ESCAPE, true);
    assert_eq!(
        rig.update_for(30, "pause"),
        [ev("pause", Started, 20), ev("pause", Triggered, 20), ev("pause", Completed, 20)]
    );

    // Pausing opens the menu while Escape is still down: it must not close it at once.
    let menu = rig.input.map().context("menu").expect("menu");
    rig.input.enable_context(menu, 10);
    assert_eq!(rig.update(40), []);
    rig.key(50, Scancode::ESCAPE, false);
    rig.key(60, Scancode::ESCAPE, true);
    assert_eq!(
        rig.update_for(70, "close"),
        [ev("close", Started, 60), ev("close", Triggered, 60), ev("close", Completed, 60)]
    );

    // Disabling the game context completes its held walk, as of the update that applies it.
    let game = rig.input.map().context("game").expect("game");
    rig.input.disable_context(game);
    assert_eq!(rig.update(80), [ev("walk", Completed, 70)]);
}

#[test]
fn modifiers_shape_values_in_the_pipeline() {
    let map = map(
        r#"(name: "move", kind: Axis2D), (name: "spin", kind: Axis1D)"#,
        r#"(name: "main", bindings: [
            (action: "move", source: Key(W), modifiers: [Swizzle]),
            (action: "move", source: Key(S), modifiers: [Swizzle, Negate(Y)]),
            (action: "move", source: Key(A), modifiers: [Negate(X)]),
            (action: "move", source: Key(D)),
            (action: "move", source: PadStick(Left), modifiers: [DeadZone(lower: 0.2)]),
            (action: "spin", source: Key(R), modifiers: [Scale((2.0, 0.0)), ScaleByDeltaTime]),
        ])"#,
    );
    let mut rig = Rig::new(map, &["main"]);
    rig.key(1, Scancode::W, true);
    rig.key(2, Scancode::D, true);
    rig.update(10);
    assert_eq!(rig.value("move"), Vec2::new(1.0, 1.0), "W and D add up");

    rig.key(11, Scancode::W, false);
    rig.key(12, Scancode::D, false);
    rig.push(13, RawInput::GamepadAdded { pad: 0, kind: GamepadType::XboxOne });
    // SDL's -0.6 is up; a radial dead zone from 0.2 maps 0.6 to 0.5.
    rig.push(14, RawInput::PadAxis { pad: 0, axis: PadAxis::LeftY, value: -0.6 });
    rig.update(20);
    let value = rig.value("move");
    assert!((value - Vec2::new(0.0, 0.5)).length() < 1e-5, "{value}");
    rig.push(21, RawInput::PadAxis { pad: 0, axis: PadAxis::LeftY, value: -0.1 });
    rig.update(30);
    assert_eq!(rig.value("move"), Vec2::ZERO, "inside the dead zone");

    // Rates scale by the update's length: 50 ms at 2 per second, then clamped to 100 ms.
    rig.key(31, Scancode::R, true);
    rig.update(80);
    assert!((rig.value("spin").x - 0.1).abs() < 1e-5);
    rig.update(580);
    assert!(
        (rig.value("spin").x - 0.2).abs() < 1e-5,
        "a long update counts as {:?}",
        sapphire_input::MAX_UPDATE_DELTA
    );
}

#[test]
fn motion_sums_per_update_and_completes_when_it_stops() {
    let map = map(
        r#"(name: "look", kind: Axis2D), (name: "zoom", kind: Axis1D)"#,
        r#"(name: "main", bindings: [
            (action: "look", source: MouseMotion),
            (action: "zoom", source: MouseWheel),
        ])"#,
    );
    let mut rig = Rig::new(map, &["main"]);
    rig.push(5, RawInput::MouseMotion { delta: Vec2::new(1.0, 2.0) });
    rig.push(10, RawInput::MouseMotion { delta: Vec2::new(3.0, 4.0) });
    assert_eq!(
        rig.update(16),
        [ev("look", Started, 5), ev("look", Triggered, 5)],
        "starts at its first motion"
    );
    assert_eq!(rig.value("look"), Vec2::new(4.0, -6.0), "the update's motion, +y up");
    rig.push(20, RawInput::MouseMotion { delta: Vec2::new(1.0, 0.0) });
    assert_eq!(rig.update(32), [], "still moving");
    assert_eq!(rig.value("look"), Vec2::new(1.0, 0.0));
    assert_eq!(rig.update(48), [ev("look", Completed, 48)]);

    rig.push(50, RawInput::MouseWheel { delta: Vec2::new(0.0, 2.0) });
    rig.update(64);
    assert_eq!(rig.value("zoom"), Vec2::new(2.0, 0.0));
}

#[test]
fn touch_gestures_drive_actions() {
    let map = map(
        r#"(name: "orbit", kind: Axis2D), (name: "zoom", kind: Axis1D), (name: "dodge", kind: Button),
           (name: "slow", kind: Button), (name: "stop", kind: Button)"#,
        r#"(name: "main", bindings: [
            (action: "orbit", source: Touch(Drag)),
            (action: "zoom", source: Touch(Pinch)),
            (action: "dodge", source: Touch(Swipe(Right)), trigger: Pressed),
            (action: "slow", source: Touch(Press), trigger: DoubleTap(seconds: 0.3)),
            (action: "stop", source: Touch(Press), trigger: Hold(seconds: 0.5)),
        ])"#,
    );
    let mut rig = Rig::new(map, &["main"]);

    // Drag: the finger moves past the slop, so the press cancels instead of releasing.
    rig.touch(10, 0, TouchPhase::Down, 100.0, 100.0);
    rig.touch(20, 0, TouchPhase::Moved, 104.0, 97.0);
    rig.touch(30, 0, TouchPhase::Moved, 130.0, 95.0);
    let events = rig.update(33);
    assert!(events.contains(&ev("orbit", Started, 20)));
    assert!(events.contains(&ev("stop", Canceled, 30)) && events.contains(&ev("slow", Canceled, 30)));
    assert_eq!(rig.value("orbit"), Vec2::new(30.0, 5.0));
    rig.touch(40, 0, TouchPhase::Up, 130.0, 95.0);
    assert_eq!(rig.update_for(50, "orbit"), [ev("orbit", Completed, 50)]);

    // Pinch: 100 points apart, spread to 200.
    rig.touch(100, 0, TouchPhase::Down, 100.0, 100.0);
    rig.touch(110, 1, TouchPhase::Down, 200.0, 100.0);
    rig.touch(120, 1, TouchPhase::Moved, 300.0, 100.0);
    rig.update(133);
    assert!((rig.value("zoom").x - 2f32.ln()).abs() < 1e-5);
    rig.touch(140, 0, TouchPhase::Up, 100.0, 100.0);
    rig.touch(150, 1, TouchPhase::Up, 300.0, 100.0);
    rig.update(166);

    // A swipe to the right dodges when the finger lifts.
    rig.touch(1000, 0, TouchPhase::Down, 100.0, 100.0);
    rig.touch(1050, 0, TouchPhase::Moved, 200.0, 100.0);
    rig.touch(1100, 0, TouchPhase::Up, 200.0, 100.0);
    assert_eq!(
        rig.update_for(1116, "dodge"),
        [ev("dodge", Started, 1100), ev("dodge", Triggered, 1100), ev("dodge", Completed, 1100)]
    );

    // A double tap, then a long press.
    for (t, phase) in
        [(2000, TouchPhase::Down), (2050, TouchPhase::Up), (2150, TouchPhase::Down), (2200, TouchPhase::Up)]
    {
        rig.touch(t, 0, phase, 500.0, 250.0);
    }
    assert!(rig.update_for(2216, "slow").contains(&ev("slow", Triggered, 2150)));
    rig.touch(3000, 0, TouchPhase::Down, 500.0, 250.0);
    assert_eq!(rig.update_for(3600, "stop"), [ev("stop", Started, 3000), ev("stop", Triggered, 3500)]);
}

#[test]
fn the_last_used_device_follows_the_player() {
    let map = map(r#"(name: "a", kind: Button)"#, r#"(name: "main", bindings: [])"#);
    let mut rig = Rig::new(map, &["main"]);
    assert_eq!(rig.input.last_device(), None);
    rig.key(1, Scancode::A, true);
    rig.update(10);
    assert_eq!(rig.input.last_device(), Some(DeviceKind::KeyboardMouse));

    rig.push(11, RawInput::GamepadAdded { pad: 2, kind: GamepadType::PlayStation5 });
    rig.push(12, RawInput::PadAxis { pad: 2, axis: PadAxis::LeftX, value: 0.1 });
    rig.update(20);
    assert_eq!(rig.input.last_device(), Some(DeviceKind::KeyboardMouse), "stick drift isn't use");
    assert_eq!(rig.input.gamepads(), 1);
    rig.push(21, RawInput::PadButton { pad: 2, button: PadButton::South, down: true });
    rig.update(30);
    assert_eq!(rig.input.last_device(), Some(DeviceKind::Gamepad(GamepadFamily::PlayStation)));

    rig.touch(31, 0, TouchPhase::Down, 10.0, 10.0);
    rig.update(40);
    assert_eq!(rig.input.last_device(), Some(DeviceKind::Touch));
    rig.push(41, RawInput::MouseMotion { delta: Vec2::new(1.0, 0.0) });
    rig.update(50);
    assert_eq!(rig.input.last_device(), Some(DeviceKind::KeyboardMouse));
}

#[test]
fn on_screen_controls_inject_through_their_bindings() {
    let map = map(
        r#"(name: "dodge", kind: Button), (name: "move", kind: Axis2D)"#,
        r#"(name: "main", bindings: [
            (action: "dodge", source: Virtual("dodge"), trigger: Pressed),
            (action: "move", source: Virtual("stick"), modifiers: [DeadZone(lower: 0.1)]),
        ])"#,
    );
    let mut rig = Rig::new(map, &["main"]);
    let dodge = rig.input.map().virtual_control("dodge").expect("dodge");
    let stick = rig.input.map().virtual_control("stick").expect("stick");
    rig.input.inject(dodge, Vec2::X, ms(10));
    rig.input.inject(stick, Vec2::new(0.0, 1.0), ms(12));
    rig.input.inject(dodge, Vec2::ZERO, ms(14));
    assert_eq!(
        rig.update_for(16, "dodge"),
        [ev("dodge", Started, 10), ev("dodge", Triggered, 10), ev("dodge", Completed, 10)]
    );
    assert_eq!(rig.value("move"), Vec2::new(0.0, 1.0));
}

#[test]
fn bindings_rebind_by_name_and_capture_the_next_press() {
    let map = map(
        r#"(name: "jump", kind: Button)"#,
        r#"(name: "main", bindings: [(action: "jump", source: Key(Space), trigger: Pressed, name: "jump.keyboard")])"#,
    );
    let mut rig = Rig::new(map, &["main"]);
    // Rebinding UI: capture what the player presses next. The press is a choice, not input.
    rig.input.capture_next();
    rig.key(10, Scancode::J, true);
    assert_eq!(rig.update(20), []);
    let captured = rig.input.take_captured().expect("captured a key");
    assert_eq!(captured, Source::Key(Key::J));
    assert_eq!(rig.input.rebind("jump.keyboard", captured).expect("rebind"), Source::Key(Key::Space));

    rig.key(30, Scancode::J, false);
    rig.key(40, Scancode::SPACE, true);
    rig.key(50, Scancode::J, true);
    assert_eq!(
        rig.update_for(60, "jump"),
        [ev("jump", Started, 50), ev("jump", Triggered, 50), ev("jump", Completed, 50)]
    );
    assert_eq!(rig.input.bindings().contexts[0].bindings[0].source, Source::Key(Key::J));
    assert!(matches!(rig.input.rebind("dodge", Source::Key(Key::K)), Err(RebindError::UnknownBinding(_))));
}

#[test]
fn losing_focus_releases_everything_without_firing_releases() {
    let map = map(
        r#"(name: "fire", kind: Button), (name: "walk", kind: Button)"#,
        r#"(name: "main", bindings: [
            (action: "fire", source: Key(F), trigger: Released),
            (action: "walk", source: Key(W)),
        ])"#,
    );
    let mut rig = Rig::new(map, &["main"]);
    rig.key(10, Scancode::F, true);
    rig.key(11, Scancode::W, true);
    rig.update(20);
    rig.input
        .handle_event(&Event::Lifecycle { timestamp: ms(25), event: LifecycleEvent::WillEnterBackground });
    assert_eq!(rig.update(30), [ev("fire", Canceled, 25), ev("walk", Completed, 25)]);
    // The real releases arriving later change nothing.
    rig.key(40, Scancode::F, false);
    assert_eq!(rig.update(50), []);
}

#[test]
fn a_removed_gamepad_lets_go_of_its_buttons() {
    let map = map(
        r#"(name: "guard", kind: Button)"#,
        r#"(name: "main", bindings: [(action: "guard", source: Pad(LeftShoulder))])"#,
    );
    let mut rig = Rig::new(map, &["main"]);
    rig.push(1, RawInput::GamepadAdded { pad: 0, kind: GamepadType::SwitchPro });
    rig.push(2, RawInput::PadButton { pad: 0, button: PadButton::LeftShoulder, down: true });
    assert_eq!(rig.update(10), [ev("guard", Started, 2), ev("guard", Triggered, 2)]);
    rig.push(12, RawInput::GamepadRemoved { pad: 0 });
    assert_eq!(rig.update(20), [ev("guard", Completed, 12)]);
    assert_eq!(rig.input.gamepads(), 0);
}
