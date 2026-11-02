//! The whole pipeline, driven with synthetic raw input and timestamps: triggers, chords,
//! context priority and consumption, modifiers, gestures, on-screen controls, rebinding, and
//! the timestamps every action event carries.

use sapphire_input::ActionEventKind::{Canceled, Completed, Started, Triggered};
use sapphire_input::{
    ActionEventKind, ActionId, DeviceKind, GamepadFamily, InputMap, InputSystem, Key, MAX_FINGERS,
    MouseButton, PadAxis, PadButton, RawInput, RebindError, Source, Timestamp,
};
use sapphire_math::Vec2;
use sapphire_platform::{
    Event, FingerId, GamepadType, KeyEvent, Keycode, LifecycleEvent, Modifiers, MouseButtonEvent, Scancode,
    TouchDeviceId, TouchEvent, TouchPhase, WindowEvent, WindowId,
};

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

    fn mouse(&mut self, t: u64, x: f32) {
        self.push(t, RawInput::MouseMotion { delta: Vec2::new(x, 0.0) });
    }

    fn left(&mut self, t: u64, down: bool) {
        self.push(t, RawInput::MouseButton { button: MouseButton::Left, down });
    }

    fn wheel(&mut self, t: u64) {
        self.push(t, RawInput::MouseWheel { delta: Vec2::new(0.0, 1.0) });
    }

    /// A platform touch at the middle of the window, as SDL reports one: by finger id, which
    /// the system maps to a slot.
    fn platform_touch(&mut self, t: u64, id: u64, phase: TouchPhase) {
        self.platform_touch_at(t, id, phase, 0.5);
    }

    /// A platform touch `x` of the way across the window, along its middle.
    fn platform_touch_at(&mut self, t: u64, id: u64, phase: TouchPhase, x: f32) {
        self.input.handle_event(&Event::Touch(TouchEvent {
            timestamp: ms(t),
            window: WindowId::from_raw(1),
            finger: FingerId::new(TouchDeviceId::from_raw(1), id),
            phase,
            x,
            y: 0.5,
            pressure: 1.0,
        }));
    }

    /// The app goes to the background at `t`.
    fn background(&mut self, t: u64) {
        self.input
            .handle_event(&Event::Lifecycle { timestamp: ms(t), event: LifecycleEvent::WillEnterBackground });
    }

    /// The window loses focus at `t`.
    fn focus_lost(&mut self, t: u64) {
        self.input.handle_event(&Event::Window {
            timestamp: ms(t),
            window: WindowId::from_raw(1),
            event: WindowEvent::FocusLost,
        });
    }

    /// A key event as the platform reports one.
    fn platform_key(&mut self, t: u64, scancode: Scancode, down: bool) {
        self.input.handle_event(&Event::Key(KeyEvent {
            timestamp: ms(t),
            window: WindowId::from_raw(1),
            scancode,
            keycode: Keycode(0),
            modifiers: Modifiers::default(),
            down,
            repeat: false,
        }));
    }

    /// The left mouse button's event as the platform reports one.
    fn platform_left(&mut self, t: u64, down: bool) {
        self.input.handle_event(&Event::MouseButton(MouseButtonEvent {
            timestamp: ms(t),
            window: WindowId::from_raw(1),
            button: sapphire_platform::MouseButton::Left,
            down,
            clicks: 1,
            x: 0.0,
            y: 0.0,
        }));
    }

    /// The window loses focus at `t` as SDL 3.4.18 reports it (`SDL_SetKeyboardFocus(NULL)`): an
    /// ordinary key-up for every key still held, then the loss, all before the next frame.
    fn focus_lost_with_keys_held(&mut self, t: u64, held: &[Scancode]) {
        for &scancode in held {
            self.platform_key(t, scancode, false);
        }
        self.focus_lost(t);
    }

    /// Whether `action` fired in the latest update.
    fn fired(&self, action: &str) -> bool {
        self.input.state(self.action(action)).fired()
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

/// One binding per trigger that a release would fire or start, and one that reads the level.
fn release_triggers_map() -> InputMap {
    map(
        r#"(name: "fire", kind: Button), (name: "light", kind: Button), (name: "dash", kind: Button),
           (name: "walk", kind: Button)"#,
        r#"(name: "main", bindings: [
            (action: "fire", source: Key(F), trigger: Released),
            (action: "light", source: Key(L), trigger: Tap(seconds: 0.2)),
            (action: "dash", source: Key(D), trigger: DoubleTap(seconds: 0.3)),
            (action: "walk", source: Key(W)),
        ])"#,
    )
}

#[test]
fn losing_focus_in_sdls_order_fires_no_release() {
    // Review WA-1: SDL lets go of every held key with ordinary key-ups just before it reports the
    // loss, so the ups must not count as the player releasing: an alt-tab fired charged and light
    // attacks. Through `handle_event`, as the app sees it.
    let mut rig = Rig::new(release_triggers_map(), &["main"]);
    let held = [Scancode::D, Scancode::F, Scancode::L, Scancode::W];
    for (at, &scancode) in held.iter().enumerate() {
        rig.platform_key(10 + at as u64, scancode, true);
    }
    rig.update(20);
    rig.focus_lost_with_keys_held(25, &held);
    assert_eq!(
        rig.update(30),
        [
            ev("fire", Canceled, 25),
            ev("light", Canceled, 25),
            ev("dash", Canceled, 25),
            ev("walk", Completed, 25),
        ]
    );
    for action in ["fire", "light", "dash"] {
        assert!(!rig.fired(action), "{action}");
    }

    // The double tap didn't arm either: a press after the return starts a new one.
    rig.platform_key(40, Scancode::D, true);
    assert_eq!(rig.update(50), [ev("dash", Started, 40)]);
    assert!(!rig.fired("dash"));

    // Bindings work as ever after the return.
    rig.platform_key(60, Scancode::F, true);
    rig.platform_key(61, Scancode::F, false);
    rig.platform_key(62, Scancode::L, true);
    rig.platform_key(63, Scancode::L, false);
    rig.update(70);
    assert!(rig.fired("fire") && rig.fired("light"));
}

#[test]
fn going_to_the_background_in_sdls_order_fires_no_release() {
    // `SDL_OnApplicationWillEnterBackground` clears the keyboard focus too, so the same ups and
    // the loss come first, and the lifecycle event after them.
    let mut rig = Rig::new(release_triggers_map(), &["main"]);
    rig.platform_key(10, Scancode::F, true);
    rig.platform_key(11, Scancode::L, true);
    rig.update(20);
    rig.focus_lost_with_keys_held(25, &[Scancode::F, Scancode::L]);
    rig.background(25);
    assert_eq!(rig.update(30), [ev("fire", Canceled, 25), ev("light", Canceled, 25)]);
}

#[test]
fn a_mouse_button_up_just_before_the_loss_fires_nothing() {
    // SDL 3.4.18 posts no mouse-up for focus (`SDL_ResetMouse` is compiled out), so usually the
    // loss comes alone and a later release finds nothing held. Wayland's pointer leave does raise
    // held buttons, and the compositor may send it just before the keyboard's leave.
    let map = map(
        r#"(name: "aim", kind: Button), (name: "flick", kind: Button), (name: "walk", kind: Button)"#,
        r#"(name: "main", bindings: [
            (action: "aim", source: Mouse(Left), trigger: Released),
            (action: "flick", source: Mouse(Left), trigger: Tap(seconds: 0.2)),
            (action: "walk", source: Key(W)),
        ])"#,
    );
    let mut rig = Rig::new(map.clone(), &["main"]);
    rig.platform_left(10, true);
    rig.platform_key(11, Scancode::W, true);
    rig.update(20);
    rig.platform_left(25, false);
    rig.focus_lost_with_keys_held(25, &[Scancode::W]);
    assert_eq!(
        rig.update(30),
        [ev("aim", Canceled, 25), ev("flick", Canceled, 25), ev("walk", Completed, 25)]
    );
    assert!(!rig.fired("aim") && !rig.fired("flick"));

    // The usual order: the loss alone, and the real release much later.
    let mut rig = Rig::new(map, &["main"]);
    rig.platform_left(10, true);
    rig.update(20);
    rig.focus_lost(25);
    assert_eq!(rig.update(30), [ev("aim", Canceled, 25), ev("flick", Canceled, 25)]);
    rig.platform_left(40, false);
    assert_eq!(rig.update(50), []);
}

#[test]
fn only_the_releases_right_before_the_loss_count_as_part_of_it() {
    // A release with other input after it is the player's.
    let mut rig = Rig::new(release_triggers_map(), &["main"]);
    rig.platform_key(10, Scancode::F, true);
    rig.platform_key(11, Scancode::L, true);
    rig.platform_key(12, Scancode::W, true);
    rig.update(20);
    rig.platform_key(21, Scancode::F, false);
    rig.platform_key(22, Scancode::H, true);
    rig.focus_lost_with_keys_held(25, &[Scancode::H, Scancode::L, Scancode::W]);
    assert_eq!(
        rig.update(30),
        [
            ev("fire", Triggered, 21),
            ev("fire", Completed, 21),
            ev("light", Canceled, 25),
            ev("walk", Completed, 25),
        ]
    );

    // Raw input replays exactly as it was pushed: only `handle_event` knows SDL's order.
    let mut rig = Rig::new(release_triggers_map(), &["main"]);
    rig.key(10, Scancode::F, true);
    rig.update(20);
    rig.key(25, Scancode::F, false);
    rig.push(25, RawInput::ReleaseAll);
    assert_eq!(rig.update(30), [ev("fire", Triggered, 25), ev("fire", Completed, 25)]);
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

#[test]
fn losing_focus_frees_the_slots_of_fingers_whose_releases_never_come() {
    // Review FI-001: a finger down when focus or the foreground goes may never be released.
    // Its id must not keep a slot, or new fingers run out of them.
    let map = map(
        r#"(name: "press", kind: Button)"#,
        r#"(name: "main", bindings: [(action: "press", source: Touch(Press), trigger: Pressed)])"#,
    );
    let mut rig = Rig::new(map, &["main"]);
    rig.input.set_window_size(1000.0, 500.0);
    let pressed = |t| [ev("press", Started, t), ev("press", Triggered, t), ev("press", Completed, t)];
    for round in 0..2 * MAX_FINGERS as u64 {
        let t = 100 * (round + 1);
        rig.platform_touch(t, round, TouchPhase::Down);
        if round % 2 == 0 {
            rig.background(t + 10);
        } else {
            rig.focus_lost(t + 10);
        }
        assert_eq!(rig.update_for(t + 20, "press"), pressed(t), "finger {round}");
    }
    // Releasing everything through the system does the same.
    for round in 0..2 * MAX_FINGERS as u64 {
        let t = 10_000 + 100 * round;
        rig.platform_touch(t, 1000 + round, TouchPhase::Down);
        rig.input.release_all(ms(t + 10));
        assert_eq!(rig.update_for(t + 20, "press"), pressed(t), "finger {}", 1000 + round);
    }
}

#[test]
fn fingers_after_a_focus_loss_in_the_same_frame_keep_their_slots() {
    // Review FI-001, within one frame: slots are freed as the loss is translated, before the
    // fingers after it land, and those keep their slots until they lift, in a later frame too.
    let map = map(
        r#"(name: "tap", kind: Button)"#,
        r#"(name: "main", bindings: [(action: "tap", source: Touch(Press), trigger: Tap(seconds: 0.2))])"#,
    );
    let mut rig = Rig::new(map, &["main"]);
    rig.input.set_window_size(1000.0, 500.0);
    let mut expected = Vec::new();
    for round in 0..2 * MAX_FINGERS as u64 {
        rig.platform_touch(10 + 2 * round, round, TouchPhase::Down);
        rig.focus_lost(11 + 2 * round);
        expected.extend([ev("tap", Started, 10 + 2 * round), ev("tap", Canceled, 11 + 2 * round)]);
    }
    rig.platform_touch(60, 500, TouchPhase::Down);
    expected.push(ev("tap", Started, 60));
    assert_eq!(rig.update(70), expected);
    rig.platform_touch(80, 500, TouchPhase::Up);
    assert_eq!(rig.update(90), [ev("tap", Triggered, 80), ev("tap", Completed, 80)]);
    // The interrupted fingers' late releases find no finger and change nothing.
    for round in 0..2 * MAX_FINGERS as u64 {
        rig.platform_touch(100, round, TouchPhase::Up);
    }
    assert_eq!(rig.update(110), []);
    rig.platform_touch(120, 600, TouchPhase::Down);
    rig.platform_touch(130, 600, TouchPhase::Up);
    assert_eq!(
        rig.update(140),
        [ev("tap", Started, 120), ev("tap", Triggered, 130), ev("tap", Completed, 130)]
    );
}

#[test]
fn losing_focus_cancels_double_taps_waiting_for_their_second_press() {
    // Review FI-003: a double tap armed by its first tap is a sequence in progress. Losing focus
    // or the foreground cancels it at once, so the first press after resuming starts a new one.
    let map = map(
        r#"(name: "slow", kind: Button), (name: "dash", kind: Button)"#,
        r#"(name: "main", bindings: [
            (action: "slow", source: Touch(Press), trigger: DoubleTap(seconds: 0.3)),
            (action: "dash", source: Key(K), trigger: DoubleTap(seconds: 0.3)),
        ])"#,
    );
    let mut rig = Rig::new(map, &["main"]);
    rig.touch(10, 0, TouchPhase::Down, 50.0, 50.0);
    rig.touch(20, 0, TouchPhase::Up, 50.0, 50.0);
    rig.key(21, Scancode::K, true);
    rig.key(22, Scancode::K, false);
    assert_eq!(rig.update(30), [ev("slow", Started, 10), ev("dash", Started, 21)]);
    rig.background(40);
    assert_eq!(
        rig.update(50),
        [ev("slow", Canceled, 40), ev("dash", Canceled, 40)],
        "canceled when focus goes, not when the window closes"
    );

    rig.touch(60, 0, TouchPhase::Down, 50.0, 50.0);
    rig.key(61, Scancode::K, true);
    assert_eq!(rig.update(70), [ev("slow", Started, 60), ev("dash", Started, 61)], "new sequences");
    // Two genuine taps after resuming still fire, once each.
    rig.touch(80, 0, TouchPhase::Up, 50.0, 50.0);
    rig.key(81, Scancode::K, false);
    rig.touch(120, 0, TouchPhase::Down, 50.0, 50.0);
    rig.key(121, Scancode::K, true);
    assert_eq!(
        rig.update(130),
        [
            ev("slow", Triggered, 120),
            ev("slow", Completed, 120),
            ev("dash", Triggered, 121),
            ev("dash", Completed, 121),
        ]
    );
}

#[test]
fn a_double_tap_across_a_focus_loss_in_one_frame_does_not_fire() {
    // Review FI-003 within one frame, through the system's own release.
    let map = map(
        r#"(name: "slow", kind: Button)"#,
        r#"(name: "main", bindings: [(action: "slow", source: Touch(Press), trigger: DoubleTap(seconds: 0.3))])"#,
    );
    let mut rig = Rig::new(map, &["main"]);
    rig.touch(10, 0, TouchPhase::Down, 50.0, 50.0);
    rig.touch(20, 0, TouchPhase::Up, 50.0, 50.0);
    rig.input.release_all(ms(30));
    rig.touch(40, 0, TouchPhase::Down, 50.0, 50.0);
    rig.touch(50, 0, TouchPhase::Up, 50.0, 50.0);
    assert_eq!(rig.update(60), [ev("slow", Started, 10), ev("slow", Canceled, 30), ev("slow", Started, 40)]);
    // The tap after the release armed a double tap of its own.
    rig.touch(100, 0, TouchPhase::Down, 50.0, 50.0);
    assert_eq!(rig.update(110), [ev("slow", Triggered, 100), ev("slow", Completed, 100)]);
}

#[test]
fn a_removed_gamepad_leaves_other_devices_sequences_alone() {
    // Review FI-003's policy: only losing focus or the foreground cancels every sequence. A pad
    // that disconnects lets go of what it held, and a keyboard double tap stays armed.
    let map = map(
        r#"(name: "dash", kind: Button), (name: "guard", kind: Button)"#,
        r#"(name: "main", bindings: [
            (action: "dash", source: Key(K), trigger: DoubleTap(seconds: 0.3)),
            (action: "guard", source: Pad(LeftShoulder)),
        ])"#,
    );
    let mut rig = Rig::new(map, &["main"]);
    rig.push(1, RawInput::GamepadAdded { pad: 0, kind: GamepadType::XboxOne });
    rig.push(2, RawInput::PadButton { pad: 0, button: PadButton::LeftShoulder, down: true });
    rig.key(10, Scancode::K, true);
    rig.key(20, Scancode::K, false);
    assert_eq!(rig.update(30), [ev("guard", Started, 2), ev("guard", Triggered, 2), ev("dash", Started, 10)]);
    rig.push(40, RawInput::GamepadRemoved { pad: 0 });
    assert_eq!(rig.update(50), [ev("guard", Completed, 40)]);
    rig.key(60, Scancode::K, true);
    assert_eq!(rig.update(70), [ev("dash", Triggered, 60), ev("dash", Completed, 60)]);
}

/// `look` reads mouse motion while `grab` (the left button) is held, and finger drags always.
/// `pan` reads mouse motion too, but `grab`'s chord blocks it while held.
fn drag_map() -> InputMap {
    map(
        r#"(name: "grab", kind: Button), (name: "look", kind: Axis2D), (name: "pan", kind: Axis2D)"#,
        r#"(name: "main", bindings: [
            (action: "grab", source: Mouse(Left)),
            (action: "look", source: MouseMotion, chord: "grab"),
            (action: "look", source: Touch(Drag)),
            (action: "pan", source: MouseMotion),
        ])"#,
    )
}

#[test]
fn a_chorded_drag_takes_only_the_motion_after_its_press() {
    // Review FI-002: motion counts toward a chorded binding only while the chord is held.
    let mut rig = Rig::new(drag_map(), &["main"]);
    rig.mouse(1, 100.0);
    rig.left(2, true);
    rig.mouse(3, 1.0);
    assert_eq!(
        rig.update_for(16, "look"),
        [ev("look", Started, 3), ev("look", Triggered, 3)],
        "the drag starts with its first motion"
    );
    assert_eq!(rig.value("look"), Vec2::new(1.0, 0.0), "the motion before the press isn't a drag");
    assert_eq!(rig.value("pan"), Vec2::new(100.0, 0.0), "it pans, until the chord blocks panning");
}

#[test]
fn a_chorded_drag_keeps_the_motion_before_its_release() {
    // Review FI-002: motion taken while the chord was held is delivered, even when the chord
    // lets go later in the same update. The action stays triggered for that update.
    let mut rig = Rig::new(drag_map(), &["main"]);
    rig.left(2, true);
    rig.mouse(3, 1.0);
    rig.left(4, false);
    rig.mouse(5, 10.0);
    assert_eq!(rig.update_for(16, "look"), [ev("look", Started, 3), ev("look", Triggered, 3)]);
    assert_eq!(rig.value("look"), Vec2::new(1.0, 0.0), "the motion while held");
    assert!(rig.input.state(rig.action("look")).is_triggered());
    assert_eq!(rig.value("pan"), Vec2::new(10.0, 0.0), "the motion after the release pans");
    assert_eq!(rig.update_for(32, "look"), [ev("look", Completed, 32)]);
}

#[test]
fn motion_adds_up_the_same_however_updates_split_it() {
    // Review FI-002: every motion counts toward the chord's state when it arrived, so the
    // per-update values add up to the same totals wherever the frame boundaries fall.
    let mouse = |x: f32| RawInput::MouseMotion { delta: Vec2::new(x, 0.0) };
    let left = |down| RawInput::MouseButton { button: MouseButton::Left, down };
    let finger = |phase, x| RawInput::Touch { finger: 0, phase, position: Vec2::new(x, 100.0) };
    let inputs = [
        (10, mouse(5.0)),
        (20, left(true)),
        (30, mouse(1.0)),
        (40, finger(TouchPhase::Down, 100.0)),
        (50, mouse(2.0)),
        (60, finger(TouchPhase::Moved, 110.0)),
        (70, mouse(3.0)),
        (80, left(false)),
        (90, mouse(7.0)),
        (100, finger(TouchPhase::Moved, 130.0)),
        (110, left(true)),
        (120, mouse(20.0)),
        (130, finger(TouchPhase::Up, 130.0)),
        (140, left(false)),
    ];
    let splits: [Vec<u64>; 4] =
        [vec![], vec![75], vec![25, 55, 85, 115], (1..15).map(|n| 10 * n + 5).collect()];
    for split in splits {
        let mut rig = Rig::new(drag_map(), &["main"]);
        let (mut look, mut pan) = (Vec2::ZERO, Vec2::ZERO);
        let mut next = 0;
        for end in split.iter().copied().chain([200, 216]) {
            while let Some(&(t, input)) = inputs.get(next).filter(|(t, _)| *t < end) {
                rig.push(t, input);
                next += 1;
            }
            rig.input.update(ms(end));
            look += rig.value("look");
            pan += rig.value("pan");
        }
        // Held: 1 + 2 + 3 and 20 of mouse motion, plus 10 + 20 of finger drag. Not held: 5 + 7.
        assert_eq!((look, pan), (Vec2::new(56.0, 0.0), Vec2::new(12.0, 0.0)), "updates ending at {split:?}");
    }
}

#[test]
fn losing_focus_judges_motion_bindings_on_what_they_took() {
    // Review FI-002 with FI-003: losing focus judges motion bindings too. A drag keeps the
    // motion it took before the loss, and a binding that has taken none yet this update goes
    // idle, so a wheel notch after the loss starts a new run.
    let map = map(
        r#"(name: "grab", kind: Button), (name: "look", kind: Axis2D), (name: "next", kind: Button)"#,
        r#"(name: "main", bindings: [
            (action: "grab", source: Mouse(Left)),
            (action: "look", source: MouseMotion, chord: "grab"),
            (action: "next", source: MouseWheel, trigger: Pressed),
        ])"#,
    );
    let mut rig = Rig::new(map, &["main"]);
    rig.left(1, true);
    rig.mouse(2, 3.0);
    rig.wheel(3);
    rig.update(16);
    assert_eq!(rig.input.state(rig.action("next")).fire_count(), 1);

    rig.mouse(18, 4.0);
    rig.focus_lost(20);
    rig.mouse(22, 50.0);
    rig.wheel(30);
    assert_eq!(
        rig.update(32),
        [
            ev("grab", Completed, 20),
            ev("next", Started, 30),
            ev("next", Triggered, 30),
            ev("next", Completed, 30)
        ]
    );
    assert_eq!(rig.value("look"), Vec2::new(4.0, 0.0), "the drag keeps the motion it took before the loss");
    assert_eq!(rig.update_for(48, "look"), [ev("look", Completed, 48)]);
}

#[test]
fn losing_focus_fires_no_release_from_motion() {
    // Review WA-4: the motion before the loss still reads actuated in the loss's own evaluation,
    // so it used to fall in the next update, as a release. The loss ends the run instead.
    let triggers = [
        ("Released", true),
        ("Tap(seconds: 0.2)", true),
        ("DoubleTap(seconds: 0.2)", true),
        ("Hold(seconds: 0.2)", true),
        ("Pressed", false),
    ];
    type Moved = fn(&mut Rig, u64);
    let sources: [(&str, Moved); 2] =
        [("MouseMotion", |rig, t| rig.mouse(t, 3.0)), ("MouseWheel", |rig, t| rig.wheel(t))];
    for (source, moved) in sources {
        for (trigger, waits) in triggers {
            let bindings = format!(
                r#"(name: "main", bindings: [(action: "flick", source: {source}, trigger: {trigger})])"#
            );
            let mut rig = Rig::new(map(r#"(name: "flick", kind: Button)"#, &bindings), &["main"]);
            let case = format!("{trigger} on {source}");
            moved(&mut rig, 20);
            rig.focus_lost(30);
            let expected = if waits {
                vec![ev("flick", Started, 20), ev("flick", Canceled, 30)]
            } else {
                vec![ev("flick", Started, 20), ev("flick", Triggered, 20), ev("flick", Completed, 20)]
            };
            assert_eq!(rig.update(40), expected, "{case}: the update of the loss");
            assert_eq!(rig.update(50), [], "{case}: the update after");
            assert_eq!(rig.update(60), [], "{case}: and later");
            assert_eq!(rig.input.state(rig.action("flick")).phase(), sapphire_input::ActionPhase::Idle);
        }
    }

    // A later run is a run like any other.
    let bindings = r#"(name: "main", bindings: [(action: "flick", source: MouseMotion, trigger: Released)])"#;
    let mut rig = Rig::new(map(r#"(name: "flick", kind: Button)"#, bindings), &["main"]);
    rig.mouse(20, 3.0);
    rig.focus_lost(30);
    rig.update(40);
    rig.update(50);
    rig.mouse(60, 3.0);
    assert_eq!(rig.update(70), [ev("flick", Started, 60)]);
    assert_eq!(rig.update(80), [ev("flick", Triggered, 80), ev("flick", Completed, 80)]);
}

#[test]
fn motion_after_the_loss_in_its_update_stays_part_of_the_lost_run() {
    // Nothing fires for it, and the run has to end before the next can begin.
    let bindings = r#"(name: "main", bindings: [
        (action: "flick", source: MouseMotion, trigger: Released),
        (action: "next", source: MouseMotion, trigger: Pressed),
    ])"#;
    let actions = r#"(name: "flick", kind: Button), (name: "next", kind: Button)"#;
    let mut rig = Rig::new(map(actions, bindings), &["main"]);
    let fired = |rig: &Rig, action: &str| rig.input.state(rig.action(action)).fire_count();
    rig.mouse(20, 3.0);
    rig.focus_lost(30);
    rig.mouse(35, 3.0);
    rig.update(40);
    assert_eq!(fired(&rig, "next"), 1, "only as the run before the loss began");
    rig.mouse(45, 3.0);
    assert_eq!(rig.update(50), [], "still the same run");
    assert_eq!(rig.update(60), [], "it ends without a release");
    rig.mouse(65, 3.0);
    rig.update(70);
    assert_eq!(fired(&rig, "next"), 1, "a new run");
}

#[test]
fn losing_focus_fires_no_release_from_a_chorded_drag() {
    // The chord lets go in the loss's own evaluation, and the gate settles there as in any other
    // (review FI-002): the binding keeps what it took while held, without a release later.
    let map = map(
        r#"(name: "grab", kind: Button), (name: "flick", kind: Button), (name: "look", kind: Axis2D)"#,
        r#"(name: "main", bindings: [
            (action: "grab", source: Mouse(Left)),
            (action: "flick", source: MouseMotion, chord: "grab", trigger: Released),
            (action: "look", source: MouseMotion, chord: "grab"),
        ])"#,
    );
    let mut rig = Rig::new(map, &["main"]);
    rig.left(1, true);
    rig.mouse(2, 3.0);
    rig.focus_lost(4);
    rig.mouse(5, 50.0);
    assert_eq!(rig.update_for(16, "flick"), [ev("flick", Started, 2), ev("flick", Canceled, 4)]);
    assert_eq!(rig.value("look"), Vec2::new(3.0, 0.0), "the motion while held, and only that");
    assert_eq!(rig.update_for(32, "flick"), []);
}

/// A rig binding `flick` to `source` with `trigger`, in a window touches are measured against.
fn touch_rig(source: &str, trigger: &str) -> Rig {
    let bindings =
        format!(r#"(name: "main", bindings: [(action: "flick", source: {source}, trigger: {trigger})])"#);
    let mut rig = Rig::new(map(r#"(name: "flick", kind: Button)"#, &bindings), &["main"]);
    rig.input.set_window_size(1000.0, 500.0);
    rig
}

#[test]
fn a_canceled_touch_fires_no_release_from_its_motion() {
    // Review WA-5: system gestures cancel touches routinely on phones. The motion before the cancel
    // still read actuated, or fell in a later evaluation with no abort, so it fired as a release.
    // One finger drags and a second makes it a pinch; the OS takes them all back.
    let triggers = ["Released", "Tap(seconds: 0.2)", "DoubleTap(seconds: 0.2)", "Hold(seconds: 0.2)"];
    let sources: [(&str, &[u64]); 2] = [("Touch(Drag)", &[1]), ("Touch(Pinch)", &[1, 2])];
    for (source, fingers) in sources {
        for trigger in triggers {
            for same_update in [true, false] {
                let case = format!("{trigger} on {source}, same update: {same_update}");
                let mut rig = touch_rig(source, trigger);
                for (at, &id) in fingers.iter().enumerate() {
                    rig.platform_touch_at(10 + at as u64, id, TouchPhase::Down, 0.1 + 0.1 * at as f32);
                }
                let last = fingers[fingers.len() - 1];
                rig.platform_touch_at(20, last, TouchPhase::Moved, 0.6);
                let mut events = Vec::new();
                if !same_update {
                    events = rig.update(25);
                }
                for &id in fingers {
                    rig.platform_touch_at(30, id, TouchPhase::Canceled, 0.6);
                }
                events.extend(rig.update(40));
                events.extend(rig.update(50));
                events.extend(rig.update(60));
                assert_eq!(events, [ev("flick", Started, 20), ev("flick", Canceled, 30)], "{case}");
            }
        }
    }

    // The next touch's run is a run like any other.
    let mut rig = touch_rig("Touch(Drag)", "Released");
    rig.platform_touch_at(10, 1, TouchPhase::Down, 0.1);
    rig.platform_touch_at(20, 1, TouchPhase::Moved, 0.6);
    rig.platform_touch_at(30, 1, TouchPhase::Canceled, 0.6);
    rig.update(40);
    rig.update(50);
    rig.platform_touch_at(60, 2, TouchPhase::Down, 0.1);
    rig.platform_touch_at(70, 2, TouchPhase::Moved, 0.6);
    assert_eq!(rig.update(80), [ev("flick", Started, 70)]);
    assert_eq!(rig.update(90), [ev("flick", Triggered, 90), ev("flick", Completed, 90)]);

    // `Down` reads the level: what the finger moved stays delivered for that update.
    let mut rig = touch_rig("Touch(Drag)", "Down");
    rig.platform_touch_at(10, 1, TouchPhase::Down, 0.1);
    rig.platform_touch_at(20, 1, TouchPhase::Moved, 0.6);
    rig.platform_touch_at(30, 1, TouchPhase::Canceled, 0.6);
    assert_eq!(rig.update(40), [ev("flick", Started, 20), ev("flick", Triggered, 20)]);
    assert_eq!(rig.update(50), [ev("flick", Completed, 50)]);
}

#[test]
fn a_canceled_touch_leaves_other_devices_sequences_alone() {
    // Only touch's own motion ends with the cancel. A double tap armed by a key, a key held for its
    // release and the mouse's run of motion all carry on and fire.
    let map = map(
        r#"(name: "drag", kind: Button), (name: "flick", kind: Button), (name: "dash", kind: Button),
           (name: "fire", kind: Button)"#,
        r#"(name: "main", bindings: [
            (action: "drag", source: Touch(Drag), trigger: Released),
            (action: "flick", source: MouseMotion, trigger: Released),
            (action: "dash", source: Key(K), trigger: DoubleTap(seconds: 0.3)),
            (action: "fire", source: Key(F), trigger: Released),
        ])"#,
    );
    let mut rig = Rig::new(map, &["main"]);
    rig.input.set_window_size(1000.0, 500.0);
    rig.key(1, Scancode::K, true);
    rig.key(2, Scancode::K, false);
    rig.key(3, Scancode::F, true);
    rig.platform_touch_at(10, 1, TouchPhase::Down, 0.1);
    rig.platform_touch_at(20, 1, TouchPhase::Moved, 0.6);
    rig.mouse(21, 3.0);
    rig.platform_touch_at(30, 1, TouchPhase::Canceled, 0.6);
    assert_eq!(
        rig.update(40),
        [
            ev("dash", Started, 1),
            ev("fire", Started, 3),
            ev("drag", Started, 20),
            ev("flick", Started, 21),
            ev("drag", Canceled, 30),
        ]
    );
    assert_eq!(rig.update(50), [ev("flick", Triggered, 50), ev("flick", Completed, 50)], "its run ends");
    rig.key(60, Scancode::K, true);
    rig.key(61, Scancode::F, false);
    assert_eq!(
        rig.update(70),
        [
            ev("dash", Triggered, 60),
            ev("dash", Completed, 60),
            ev("fire", Triggered, 61),
            ev("fire", Completed, 61),
        ]
    );
}

#[test]
fn only_the_os_taking_a_finger_back_ends_touchs_motion() {
    // A finger lifting is the player letting go, past the tap slop and all.
    let mut rig = touch_rig("Touch(Drag)", "Released");
    rig.platform_touch_at(10, 1, TouchPhase::Down, 0.1);
    rig.platform_touch_at(20, 1, TouchPhase::Moved, 0.6);
    rig.platform_touch_at(30, 1, TouchPhase::Up, 0.6);
    assert_eq!(rig.update(40), [ev("flick", Started, 20)]);
    assert_eq!(rig.update(50), [ev("flick", Triggered, 50), ev("flick", Completed, 50)]);

    // A late cancel of a finger no longer down says nothing of the fingers that are, however it
    // arrives: one with no slot is dropped, and a raw one finds no finger.
    let mut rig = touch_rig("Touch(Drag)", "Released");
    rig.platform_touch_at(10, 1, TouchPhase::Down, 0.1);
    rig.platform_touch_at(20, 1, TouchPhase::Moved, 0.6);
    rig.platform_touch_at(21, 99, TouchPhase::Canceled, 0.6);
    rig.touch(22, 5, TouchPhase::Canceled, 0.0, 0.0);
    assert_eq!(rig.update(40), [ev("flick", Started, 20)]);
    assert_eq!(rig.update(50), [ev("flick", Triggered, 50), ev("flick", Completed, 50)]);
}

#[test]
fn a_motion_pulse_fires_once_however_many_inputs_follow_it() {
    // Review FI-006: evaluating the bindings for some other input must not replay a motion
    // binding's pulse.
    let map = map(
        r#"(name: "next", kind: Button)"#,
        r#"(name: "main", bindings: [(action: "next", source: MouseWheel, trigger: Pressed)])"#,
    );
    for others in [0, 1, 10] {
        let mut rig = Rig::new(map.clone(), &["main"]);
        rig.wheel(10);
        for n in 0..others {
            rig.key(11 + 2 * n, Scancode::H, true);
            rig.key(12 + 2 * n, Scancode::H, false);
        }
        assert_eq!(
            rig.update(100),
            [ev("next", Started, 10), ev("next", Triggered, 10), ev("next", Completed, 10)],
            "with {others} other inputs"
        );
        assert_eq!(rig.input.state(rig.action("next")).fire_count(), 1);
    }
}

#[test]
fn edge_triggers_on_motion_fire_once_per_run_of_updates_with_motion() {
    // Review FI-006: motion is an impulse. A binding is actuated in the updates that gave it
    // motion, so a pressed trigger fires as a run of such updates begins and a released one as
    // it ends, whatever other input arrives, and nothing fires again until the next run.
    let map = map(
        r#"(name: "next", kind: Button), (name: "settle", kind: Button)"#,
        r#"(name: "main", bindings: [
            (action: "next", source: MouseWheel, trigger: Pressed),
            (action: "settle", source: MouseWheel, trigger: Released),
        ])"#,
    );
    let mut rig = Rig::new(map, &["main"]);
    let fired = |rig: &Rig| {
        (rig.input.state(rig.action("next")).fire_count(), rig.input.state(rig.action("settle")).fire_count())
    };
    let other = |rig: &mut Rig, t| {
        rig.key(t, Scancode::H, true);
        rig.key(t + 1, Scancode::H, false);
    };
    rig.wheel(10);
    other(&mut rig, 11);
    rig.update(16);
    assert_eq!(fired(&rig), (1, 0), "a run begins");
    rig.wheel(20);
    other(&mut rig, 21);
    rig.update(32);
    assert_eq!(fired(&rig), (0, 0), "the same run");
    other(&mut rig, 40);
    assert_eq!(
        rig.update_for(48, "settle"),
        [ev("settle", Triggered, 48), ev("settle", Completed, 48)],
        "the run ends with the first update without motion"
    );
    other(&mut rig, 50);
    rig.update(64);
    assert_eq!(fired(&rig), (0, 0), "nothing replays the release");
    rig.wheel(70);
    rig.update(80);
    assert_eq!(fired(&rig), (1, 0), "a new run");
}
