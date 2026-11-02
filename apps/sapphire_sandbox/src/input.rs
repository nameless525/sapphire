//! The sandbox's input: `sapphire_input` with the default bindings, and what the sandbox does
//! with their actions.
//!
//! - **Bindings** are two documents compiled into the app and compiled into one map: the sandbox's
//!   own, `default_bindings.ron`, and combat's (`sapphire_controls`). Embedded rather than cooked:
//!   the app must stay controllable (and quittable) when content fails to load, the bindings don't
//!   differ per cook, and player rebinds will come from user settings on top of them (M3) rather
//!   than replace them.
//! - **Camera input is presentation.** `orbit` and `zoom` apply once per frame on the real
//!   clock, so the camera stays responsive in slow motion.
//! - **Simulation input is queued.** `slow_motion` and `hitstop` become [`SimActions`] that the
//!   next fixed step consumes, so an input lands on a tick rather than whenever the OS delivered
//!   it (spec §4.2 step 1, rule R3), and replays stay deterministic.
//! - **Combat's controls** sit under the sandbox's contexts, so the sandbox's bindings take whatever
//!   source they share, and with `--combat-input` the sandbox's are off. Their ticks drive a
//!   recording's simulation under `--record`, and otherwise nothing yet, since there's no fight to
//!   play: dev builds log them. Their pause stops the fixed steps in every build, and Android's Back
//!   opens it and then leaves through it.

use std::time::Duration;

use sapphire_controls::{CombatControls, ControlsSource, default_bindings};
use sapphire_gameplay::ActorId;
use sapphire_input::{ActionEventKind, ActionId, InputMap};
use sapphire_math::Vec2;
use sapphire_platform::{AppInitError, Event, Size, Timestamp};
use sapphire_time::{FixedStepDriver, FixedSteps};

use crate::LOG_TARGET;
use crate::orbit::OrbitCamera;

/// The default bindings document.
const BINDINGS: &str = include_str!("../default_bindings.ron");

/// Contexts enabled everywhere, with their priorities, which fall between combat's and the pause's.
/// Their sources don't overlap yet.
const CONTEXTS: [(&str, i32); 2] = [("camera", 10), ("debug", 20)];
/// Enabled on desktop only: `Esc` quits there.
const DESKTOP_CONTEXT: (&str, i32) = ("desktop", 30);
/// Enabled with `--fight`.
const FIGHT_CONTEXT: (&str, i32) = ("fight", 40);

/// The actor the sandbox's ticks name when there's no scenario to say: the one a simulation spawns
/// first.
pub(crate) const PLAYER: ActorId = ActorId::from_raw(1).expect("not zero");

/// Simulation actions requested since the last fixed step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SimActions {
    /// Flip world slow motion. Two presses before a step cancel out.
    pub toggle_slow_motion: bool,
    /// Freeze the turntable for a few ticks.
    pub hitstop: bool,
    /// Start `--fight`'s fight over.
    pub restart_fight: bool,
}

/// The actions the sandbox reads.
#[derive(Clone, Copy, Debug)]
struct Actions {
    orbit: ActionId,
    zoom: ActionId,
    slow_motion: ActionId,
    hitstop: ActionId,
    quit: ActionId,
    restart_fight: ActionId,
}

pub(crate) struct SandboxInput {
    controls: CombatControls,
    actions: Actions,
    queued: SimActions,
    /// The pause as last logged.
    paused: bool,
}

impl SandboxInput {
    /// Input from the default bindings, for a window `window_size` points large. `desktop`
    /// enables the desktop-only context. `combat_input` leaves combat's controls alone with the
    /// player, without the sandbox's bindings over them.
    pub fn new(desktop: bool, combat_input: bool, window_size: Size) -> Result<Self, AppInitError> {
        let bindings =
            |error: &dyn std::fmt::Display| AppInitError::msg(format!("default bindings: {error}"));
        let own = InputMap::from_ron(BINDINGS).map_err(|error| bindings(&error))?;
        let mut document = default_bindings().map_err(|error| bindings(&error))?;
        document.actions.extend_from_slice(&own.document().actions);
        document.contexts.extend_from_slice(&own.document().contexts);
        let map = InputMap::new(document).map_err(|error| bindings(&error))?;
        let action = |name: &str| {
            map.action(name)
                .ok_or_else(|| AppInitError::msg(format!("default bindings have no `{name}` action")))
        };
        let actions = Actions {
            orbit: action("orbit")?,
            zoom: action("zoom")?,
            slow_motion: action("slow_motion")?,
            hitstop: action("hitstop")?,
            quit: action("quit")?,
            restart_fight: action("restart_fight")?,
        };
        let mut controls = CombatControls::new(map).map_err(|error| bindings(&error))?;
        let system = controls.input_mut();
        if !combat_input {
            for (name, priority) in CONTEXTS.into_iter().chain(desktop.then_some(DESKTOP_CONTEXT)) {
                let context = system
                    .map()
                    .context(name)
                    .ok_or_else(|| AppInitError::msg(format!("default bindings have no `{name}` context")))?;
                system.enable_context(context, priority);
            }
        }
        system.set_window_size(window_size.width as f32, window_size.height as f32);
        Ok(Self { controls, actions, queued: SimActions::default(), paused: false })
    }

    /// Listens for the key that starts `--fight`'s fight over, in a context of its own: pausing takes
    /// combat's bindings away and leaves this one, and [`Self::update`] ignores it while paused.
    pub fn listen_for_fight_restart(&mut self) -> Result<(), AppInitError> {
        let system = self.controls.input_mut();
        let context = system.map().context(FIGHT_CONTEXT.0).ok_or_else(|| {
            AppInitError::msg(format!("default bindings have no `{}` context", FIGHT_CONTEXT.0))
        })?;
        system.enable_context(context, FIGHT_CONTEXT.1);
        Ok(())
    }

    /// Queues a platform event for the next [`Self::update`].
    pub fn handle_event(&mut self, event: &Event) {
        self.controls.handle_event(event);
    }

    /// Processes the frame's input as of `now` (the platform clock) and queues the simulation
    /// actions it triggered. Returns whether the player asked to quit, or left from the pause.
    pub fn update(&mut self, now: Timestamp) -> bool {
        self.controls.update(now);
        let system = self.controls.input();
        for event in system.events() {
            if event.kind != ActionEventKind::Triggered {
                continue;
            }
            if event.action == self.actions.slow_motion {
                self.queued.toggle_slow_motion ^= true;
            } else if event.action == self.actions.hitstop {
                self.queued.hitstop = true;
            } else if event.action == self.actions.restart_fight && !self.controls.is_paused() {
                self.queued.restart_fight = true;
            }
        }
        if self.paused != self.controls.is_paused() {
            self.paused = self.controls.is_paused();
            tracing::info!(target: LOG_TARGET, paused = self.paused, "pause");
        }
        system.state(self.actions.quit).fired() || self.controls.wants_to_leave()
    }

    /// The pad in the player's hands, which rumble goes to.
    pub fn active_gamepad(&self) -> Option<sapphire_platform::GamepadId> {
        self.controls.input().active_gamepad()
    }

    /// This frame's fixed steps: none while paused, and no burst after.
    pub fn advance(&self, driver: &mut FixedStepDriver, frame_time: Duration) -> FixedSteps {
        self.controls.advance(driver, frame_time)
    }

    /// The bindings as the source of the fixed steps' input, turned by where the camera looks
    /// (`OrbitCamera::heading`). Asked once per step: this is where a recording takes its ticks.
    pub fn source(&mut self, camera_yaw: f32) -> ControlsSource<'_> {
        self.controls.source(camera_yaw)
    }

    /// This frame's orbit and zoom, the sandbox's own or combat's: radians of yaw and pitch, and the
    /// log of a factor on the distance. Sources go to one or the other, by context, so they add.
    pub fn camera_motion(&self) -> (Vec2, f32) {
        let system = self.controls.input();
        let orbit = system.state(self.actions.orbit).value() + self.controls.look();
        (orbit, system.state(self.actions.zoom).axis() + self.controls.camera_zoom())
    }

    /// Applies this frame's orbit and zoom.
    pub fn drive_camera(&self, camera: &mut OrbitCamera) {
        let (orbit, zoom) = self.camera_motion();
        if orbit != Vec2::ZERO {
            camera.rotate(orbit.x, orbit.y);
        }
        if zoom != 0.0 {
            camera.zoom(zoom.exp());
        }
    }

    /// Takes the actions queued for the next fixed step.
    pub fn take_actions(&mut self) -> SimActions {
        std::mem::take(&mut self.queued)
    }

    #[cfg(feature = "dev")]
    pub fn last_device(&self) -> Option<sapphire_input::DeviceKind> {
        self.controls.input().last_device()
    }

    #[cfg(feature = "dev")]
    pub fn gamepads(&self) -> usize {
        self.controls.input().gamepads()
    }
}

#[cfg(test)]
mod tests {
    use sapphire_gameplay::Buttons;
    use sapphire_gameplay::replay::ActionSource;
    use sapphire_input::{PadAxis, PadButton, RawInput};
    use sapphire_math::Vec3;
    use sapphire_platform::{GamepadType, LifecycleEvent, Scancode, TouchPhase};

    use super::*;

    fn ms(ms: u64) -> Timestamp {
        Timestamp::from_nanos(ms * 1_000_000)
    }

    fn input(desktop: bool) -> SandboxInput {
        input_with(desktop, false)
    }

    fn input_with(desktop: bool, combat_input: bool) -> SandboxInput {
        let mut input = SandboxInput::new(desktop, combat_input, Size { width: 1000, height: 500 })
            .expect("default bindings");
        input.update(ms(0));
        input
    }

    impl SandboxInput {
        pub(crate) fn push(&mut self, t: u64, raw: RawInput) {
            self.controls.input_mut().push(ms(t), raw);
        }

        fn key(&mut self, t: u64, scancode: Scancode, down: bool) {
            self.push(t, RawInput::Key { scancode, down });
        }

        fn orbit(&self) -> Vec2 {
            self.controls.input().state(self.actions.orbit).value()
        }

        /// The distance factor this frame's zoom applies.
        fn zoom_factor(&self) -> f32 {
            self.controls.input().state(self.actions.zoom).axis().exp()
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn presses_queue_actions_for_the_next_step() {
        let mut input = input(true);
        input.key(1, Scancode::SPACE, true);
        input.push(2, RawInput::GamepadAdded { pad: 0, kind: GamepadType::XboxOne });
        input.push(3, RawInput::PadButton { pad: 0, button: PadButton::East, down: true });
        assert!(!input.update(ms(16)));
        assert_eq!(
            input.take_actions(),
            SimActions { toggle_slow_motion: true, hitstop: true, restart_fight: false }
        );
        assert_eq!(input.take_actions(), SimActions::default());

        // Two toggles before a step cancel out.
        input.key(20, Scancode::SPACE, false);
        input.key(21, Scancode::SPACE, true);
        input.push(22, RawInput::PadButton { pad: 0, button: PadButton::South, down: true });
        input.update(ms(32));
        assert!(!input.take_actions().toggle_slow_motion);
    }

    #[test]
    fn releasing_everything_drops_held_input_but_keeps_queued_actions() {
        let mut input = input(true);
        input.key(1, Scancode::LEFT, true);
        input.key(2, Scancode::H, true);
        input.update(ms(16));
        assert!(input.orbit().x > 0.0);
        input.controls.input_mut().release_all(ms(20));
        input.update(ms(32));
        assert_eq!(input.orbit(), Vec2::ZERO);
        assert!(input.take_actions().hitstop);
    }

    #[test]
    fn camera_rates_match_the_m0_controls() {
        let mut input = input(true);
        // Arrow keys: 1.5 rad/s, left orbits positive yaw; up pitches positive.
        input.key(1, Scancode::LEFT, true);
        input.key(2, Scancode::UP, true);
        input.update(ms(50));
        assert!(
            close(input.orbit().x, 1.5 * 0.05) && close(input.orbit().y, 1.5 * 0.05),
            "{}",
            input.orbit()
        );
        input.key(51, Scancode::LEFT, false);
        input.key(52, Scancode::UP, false);
        input.update(ms(60));

        // The mouse orbits only while the left button is held: 0.008 rad per point.
        input.push(61, RawInput::MouseMotion { delta: Vec2::new(10.0, 5.0) });
        input.update(ms(70));
        assert_eq!(input.orbit(), Vec2::ZERO);
        input.push(71, RawInput::MouseButton { button: sapphire_input::MouseButton::Left, down: true });
        input.push(72, RawInput::MouseMotion { delta: Vec2::new(10.0, 5.0) });
        input.update(ms(80));
        assert!(close(input.orbit().x, -0.08) && close(input.orbit().y, 0.04), "{}", input.orbit());

        // One wheel notch away is 0.9× the distance.
        input.push(81, RawInput::MouseWheel { delta: Vec2::new(0.0, 1.0) });
        input.update(ms(90));
        assert!(close(input.zoom_factor(), 0.9));

        // Right stick, past its 0.15 axial dead zone, at 2.5 rad/s; SDL's -1 is up.
        input.push(91, RawInput::GamepadAdded { pad: 0, kind: GamepadType::Standard });
        input.push(92, RawInput::PadAxis { pad: 0, axis: PadAxis::RightX, value: 1.0 });
        input.push(93, RawInput::PadAxis { pad: 0, axis: PadAxis::RightY, value: -0.1 });
        input.update(ms(100));
        assert!(close(input.orbit().x, -2.5 * 0.01) && input.orbit().y == 0.0, "{}", input.orbit());
        input.push(101, RawInput::PadAxis { pad: 0, axis: PadAxis::RightX, value: 0.0 });

        // Full left trigger: 3× the distance per second.
        input.push(102, RawInput::PadAxis { pad: 0, axis: PadAxis::LeftTrigger, value: 1.0 });
        input.update(ms(200));
        assert!(close(input.zoom_factor(), 3f32.powf(0.1)));
    }

    #[test]
    fn a_mouse_drag_orbits_by_the_motion_made_while_the_button_is_held() {
        // Review FI-002: within one frame, motion before the press doesn't orbit, and motion
        // before the release still does.
        let mut input = input(true);
        let left = |down| RawInput::MouseButton { button: sapphire_input::MouseButton::Left, down };
        input.push(1, RawInput::MouseMotion { delta: Vec2::new(100.0, 0.0) });
        input.push(2, left(true));
        input.push(3, RawInput::MouseMotion { delta: Vec2::new(10.0, 5.0) });
        input.update(ms(16));
        assert!(close(input.orbit().x, -0.08) && close(input.orbit().y, 0.04), "{}", input.orbit());

        input.push(20, RawInput::MouseMotion { delta: Vec2::new(10.0, 5.0) });
        input.push(21, left(false));
        input.push(22, RawInput::MouseMotion { delta: Vec2::new(100.0, 0.0) });
        input.update(ms(32));
        assert!(close(input.orbit().x, -0.08) && close(input.orbit().y, 0.04), "{}", input.orbit());
        input.update(ms(48));
        assert_eq!(input.orbit(), Vec2::ZERO);
    }

    #[test]
    fn touch_orbits_zooms_and_reaches_the_debug_actions() {
        let mut input = input(false);
        let touch = |input: &mut SandboxInput, t, finger, phase, x, y| {
            input.push(t, RawInput::Touch { finger, phase, position: Vec2::new(x, y) });
        };
        // One finger: 0.006 rad per point, dragging right orbits negative yaw.
        touch(&mut input, 1, 0, TouchPhase::Down, 500.0, 250.0);
        touch(&mut input, 2, 0, TouchPhase::Moved, 520.0, 250.0);
        input.update(ms(16));
        assert!(close(input.orbit().x, -0.12) && input.orbit().y == 0.0, "{}", input.orbit());

        // A second finger pinches: spreading from 100 to 200 points halves the distance.
        touch(&mut input, 20, 1, TouchPhase::Down, 620.0, 250.0);
        touch(&mut input, 21, 1, TouchPhase::Moved, 720.0, 250.0);
        input.update(ms(32));
        assert!(close(input.zoom_factor(), 0.5));
        assert_eq!(input.orbit(), Vec2::ZERO, "two fingers don't orbit");
        touch(&mut input, 40, 0, TouchPhase::Up, 520.0, 250.0);
        touch(&mut input, 41, 1, TouchPhase::Up, 720.0, 250.0);
        input.update(ms(48));

        // A double tap toggles slow motion; a long press is a hitstop.
        for (t, phase) in
            [(100, TouchPhase::Down), (150, TouchPhase::Up), (250, TouchPhase::Down), (300, TouchPhase::Up)]
        {
            touch(&mut input, t, 0, phase, 300.0, 300.0);
        }
        input.update(ms(316));
        assert_eq!(
            input.take_actions(),
            SimActions { toggle_slow_motion: true, hitstop: false, restart_fight: false }
        );
        touch(&mut input, 1000, 0, TouchPhase::Down, 300.0, 300.0);
        input.update(ms(1600));
        assert_eq!(
            input.take_actions(),
            SimActions { toggle_slow_motion: false, hitstop: true, restart_fight: false }
        );
    }

    #[test]
    fn leaving_the_foreground_cancels_a_half_finished_double_tap() {
        // Review FI-003: a tap, then the app leaves the foreground; the first press after it
        // comes back starts a new double tap rather than finishing the old one.
        let mut input = input(false);
        let touch = |input: &mut SandboxInput, t, phase| {
            input.push(t, RawInput::Touch { finger: 0, phase, position: Vec2::new(300.0, 300.0) });
        };
        touch(&mut input, 10, TouchPhase::Down);
        touch(&mut input, 60, TouchPhase::Up);
        input.update(ms(70));
        input.handle_event(&Event::Lifecycle {
            timestamp: ms(80),
            event: LifecycleEvent::WillEnterBackground,
        });
        input.update(ms(90));
        touch(&mut input, 100, TouchPhase::Down);
        touch(&mut input, 150, TouchPhase::Up);
        input.update(ms(160));
        assert_eq!(input.take_actions(), SimActions::default(), "no slow motion");
    }

    #[test]
    fn escape_quits_on_desktop_only() {
        let mut desktop = input(true);
        desktop.key(1, Scancode::ESCAPE, true);
        assert!(desktop.update(ms(16)));
        assert!(!desktop.update(ms(32)), "once per press");

        let mut mobile = input(false);
        mobile.key(1, Scancode::ESCAPE, true);
        assert!(!mobile.update(ms(16)));
    }

    /// What the next tick gives the actor it names.
    fn next_buttons(input: &mut SandboxInput) -> Buttons {
        input.source(0.0).actions(0, &[PLAYER]).get(PLAYER).buttons
    }

    /// The camera the app hands the mapping is the one it draws: forward walks away from the view,
    /// and right walks to the right of the picture, read from the view matrix itself.
    #[test]
    fn a_push_goes_where_the_view_goes() {
        for orbit in [0.0, 0.35, 1.0, std::f32::consts::FRAC_PI_2, 2.5, -2.0, 40.0] {
            let mut camera = OrbitCamera::framing([[-1.0, 0.0, -1.0], [1.0, 2.0, 1.0]]);
            camera.rotate(orbit - 0.35, 0.2);
            for _ in 0..600 {
                camera.update(1.0 / 60.0);
            }
            let view = camera.camera_data(16.0 / 9.0).view;
            let right = view.row(0).truncate();
            let forward = -view.row(2).truncate();
            let ground = Vec3::new(forward.x, 0.0, forward.z).normalize();

            for (key, want) in [(Scancode::W, ground), (Scancode::D, right), (Scancode::S, -ground)] {
                let mut input = input_with(false, true);
                input.key(1, key, true);
                input.update(ms(16));
                let tick = input.source(camera.heading()).actions(0, &[PLAYER]);
                let stick = tick.get(PLAYER).stick.to_vec2();
                let got = Vec3::new(stick.x, 0.0, stick.y);
                assert!(got.distance(want) < 0.02, "{key:?} at orbit {orbit}: {got}, the view's {want}");
            }
        }
    }

    #[test]
    fn combat_sits_under_the_sandboxs_bindings() {
        let mut input = input(true);
        input.key(1, Scancode::J, true);
        input.push(2, RawInput::GamepadAdded { pad: 0, kind: GamepadType::XboxOne });
        input.push(3, RawInput::PadButton { pad: 0, button: PadButton::North, down: true });
        input.push(4, RawInput::PadButton { pad: 0, button: PadButton::South, down: true });
        input.update(ms(16));
        assert_eq!(next_buttons(&mut input), Buttons::LIGHT | Buttons::SKILL, "sources it alone binds");
        assert!(input.take_actions().toggle_slow_motion, "South is still the sandbox's");

        // Its camera too, or the sandbox's camera would turn twice.
        input.key(17, Scancode::LEFT, true);
        input.push(18, RawInput::PadAxis { pad: 0, axis: PadAxis::RightX, value: 1.0 });
        input.push(19, RawInput::MouseButton { button: sapphire_input::MouseButton::Left, down: true });
        input.push(20, RawInput::MouseMotion { delta: Vec2::new(10.0, 5.0) });
        input.push(21, RawInput::MouseWheel { delta: Vec2::new(0.0, 1.0) });
        input.update(ms(32));
        assert_ne!(input.orbit(), Vec2::ZERO);
        assert_eq!(input.controls.look(), Vec2::ZERO);
        assert_eq!(input.controls.camera_zoom(), 0.0);
    }

    /// `--fight`'s key starts it over, once a press, and not from the pause, whose steps have stopped.
    /// Nothing else hears it: without `--fight` it does nothing, and combat binds neither source.
    #[test]
    fn the_fight_starts_over_on_its_own_key() {
        let mut input = input_with(true, true);
        input.push(1, RawInput::Key { scancode: Scancode::R, down: true });
        assert!(!input.update(ms(16)));
        assert!(!input.take_actions().restart_fight, "not without --fight");
        input.push(17, RawInput::Key { scancode: Scancode::R, down: false });
        assert!(!input.update(ms(32)));

        input.listen_for_fight_restart().expect("it's bound");
        input.push(33, RawInput::Key { scancode: Scancode::R, down: true });
        assert!(!input.update(ms(48)));
        assert_eq!(next_buttons(&mut input), Buttons::NONE, "it's no verb of combat's");
        assert!(input.take_actions().restart_fight);
        assert!(!input.update(ms(64)));
        assert!(!input.take_actions().restart_fight, "held, it's one press");
        input.push(65, RawInput::Key { scancode: Scancode::R, down: false });
        assert!(!input.update(ms(80)));

        input.push(80, RawInput::GamepadAdded { pad: 0, kind: GamepadType::XboxOne });
        input.push(81, RawInput::PadButton { pad: 0, button: PadButton::DpadDown, down: true });
        assert!(!input.update(ms(96)));
        assert!(input.take_actions().restart_fight, "and on a pad");
        input.push(97, RawInput::PadButton { pad: 0, button: PadButton::DpadDown, down: false });
        assert!(!input.update(ms(112)));

        input.push(113, RawInput::Key { scancode: Scancode::ESCAPE, down: true });
        assert!(!input.update(ms(128)));
        input.push(129, RawInput::Key { scancode: Scancode::ESCAPE, down: false });
        input.push(130, RawInput::Key { scancode: Scancode::R, down: true });
        assert!(!input.update(ms(144)));
        assert!(!input.take_actions().restart_fight, "not from the pause");
    }

    #[test]
    fn combat_input_takes_the_sandboxs_bindings_off() {
        let mut input = input_with(true, true);
        input.key(1, Scancode::SPACE, true);
        input.key(2, Scancode::H, true);
        assert!(!input.update(ms(16)));
        assert_eq!(next_buttons(&mut input), Buttons::JUMP, "Space is combat's now");
        assert_eq!(input.take_actions(), SimActions::default());

        // The camera is combat's too, and Esc is the pause's way in.
        input.key(17, Scancode::LEFT, true);
        input.update(ms(32));
        assert_eq!(input.orbit(), Vec2::ZERO);
        assert!(input.controls.look().x > 0.0);
        input.key(33, Scancode::ESCAPE, true);
        assert!(!input.update(ms(48)), "Esc pauses instead of quitting");
        assert!(input.controls.is_paused());
    }

    #[test]
    fn pausing_stops_the_steps() {
        let mut input = input(true);
        let mut driver = FixedStepDriver::default();
        let frame = Duration::from_millis(17);
        assert_eq!(input.advance(&mut driver, frame).count, 1);

        input.key(1, Scancode::P, true);
        input.update(ms(16));
        for _ in 0..100 {
            assert_eq!(input.advance(&mut driver, frame).count, 0);
        }
        input.key(20, Scancode::P, false);
        input.key(21, Scancode::P, true);
        input.update(ms(32));
        input.update(ms(48));
        assert_eq!(input.advance(&mut driver, frame).count, 1, "no burst");
    }

    #[test]
    fn escape_resumes_while_paused_then_quits_again() {
        let mut input = input(true);
        input.key(1, Scancode::P, true);
        input.key(2, Scancode::P, false);
        input.update(ms(16));
        assert!(input.controls.is_paused());
        input.key(17, Scancode::ESCAPE, true);
        assert!(!input.update(ms(32)), "the pause takes Esc from the desktop's quit");
        input.key(33, Scancode::ESCAPE, false);
        input.update(ms(48));
        assert!(!input.controls.is_paused());
        input.key(49, Scancode::ESCAPE, true);
        assert!(input.update(ms(64)), "Esc quits again");
    }

    #[test]
    fn android_back_opens_the_pause_then_leaves_with_every_context_up() {
        // On a phone, and with the sandbox's contexts over combat's.
        const BACK: Scancode = Scancode(282);
        let mut input = input(false);
        input.key(1, BACK, true);
        input.key(2, BACK, false);
        assert!(!input.update(ms(16)), "the first Back pauses");
        assert!(input.controls.is_paused());
        input.key(17, BACK, true);
        input.key(18, BACK, false);
        assert!(input.update(ms(32)), "the second leaves");
    }
}
