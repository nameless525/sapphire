//! The sandbox's input: `sapphire_input` with the default bindings, and what the sandbox does
//! with their actions.
//!
//! - **Bindings** are the document `default_bindings.ron`, compiled into the app. Embedded
//!   rather than cooked: the app must stay controllable (and quittable) when content fails to
//!   load, the bindings don't differ per cook, and player rebinds will come from user settings
//!   on top of them (M3) rather than replace them.
//! - **Camera input is presentation.** `orbit` and `zoom` apply once per frame on the real
//!   clock, so the camera stays responsive in slow motion.
//! - **Simulation input is queued.** `slow_motion` and `hitstop` become [`SimActions`] that the
//!   next fixed step consumes, so an input lands on a tick rather than whenever the OS delivered
//!   it (spec §4.2 step 1, rule R3), and replays stay deterministic.

use sapphire_input::{ActionEventKind, ActionId, InputMap, InputSystem};
use sapphire_math::Vec2;
use sapphire_platform::{AppInitError, Event, Size, Timestamp};

use crate::orbit::OrbitCamera;

/// The default bindings document.
const BINDINGS: &str = include_str!("../default_bindings.ron");

/// Contexts enabled everywhere, with their priorities. Their sources don't overlap yet.
const CONTEXTS: [(&str, i32); 2] = [("camera", 0), ("debug", 10)];
/// Enabled on desktop only: `Esc` quits there.
const DESKTOP_CONTEXT: (&str, i32) = ("desktop", 20);

/// Simulation actions requested since the last fixed step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SimActions {
    /// Flip world slow motion. Two presses before a step cancel out.
    pub toggle_slow_motion: bool,
    /// Freeze the turntable for a few ticks.
    pub hitstop: bool,
}

/// The actions the sandbox reads.
#[derive(Clone, Copy, Debug)]
struct Actions {
    orbit: ActionId,
    zoom: ActionId,
    slow_motion: ActionId,
    hitstop: ActionId,
    quit: ActionId,
}

pub(crate) struct SandboxInput {
    system: InputSystem,
    actions: Actions,
    queued: SimActions,
}

impl SandboxInput {
    /// Input from the default bindings, for a window `window_size` points large. `desktop`
    /// enables the desktop-only context.
    pub fn new(desktop: bool, window_size: Size) -> Result<Self, AppInitError> {
        let map = InputMap::from_ron(BINDINGS)
            .map_err(|error| AppInitError::msg(format!("default bindings: {error}")))?;
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
        };
        let mut system = InputSystem::new(map);
        for (name, priority) in CONTEXTS.into_iter().chain(desktop.then_some(DESKTOP_CONTEXT)) {
            let context = system
                .map()
                .context(name)
                .ok_or_else(|| AppInitError::msg(format!("default bindings have no `{name}` context")))?;
            system.enable_context(context, priority);
        }
        system.set_window_size(window_size.width as f32, window_size.height as f32);
        Ok(Self { system, actions, queued: SimActions::default() })
    }

    /// Queues a platform event for the next [`Self::update`].
    pub fn handle_event(&mut self, event: &Event) {
        self.system.handle_event(event);
    }

    /// Processes the frame's input as of `now` (the platform clock) and queues the simulation
    /// actions it triggered. Returns whether the player asked to quit.
    pub fn update(&mut self, now: Timestamp) -> bool {
        self.system.update(now);
        for event in self.system.events() {
            if event.kind != ActionEventKind::Triggered {
                continue;
            }
            if event.action == self.actions.slow_motion {
                self.queued.toggle_slow_motion ^= true;
            } else if event.action == self.actions.hitstop {
                self.queued.hitstop = true;
            }
        }
        self.system.state(self.actions.quit).fired()
    }

    /// Applies this frame's orbit and zoom.
    pub fn drive_camera(&self, camera: &mut OrbitCamera) {
        let orbit = self.system.state(self.actions.orbit).value();
        if orbit != Vec2::ZERO {
            camera.rotate(orbit.x, orbit.y);
        }
        let zoom = self.system.state(self.actions.zoom).axis();
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
        self.system.last_device()
    }

    #[cfg(feature = "dev")]
    pub fn gamepads(&self) -> usize {
        self.system.gamepads()
    }
}

#[cfg(test)]
mod tests {
    use sapphire_input::{PadAxis, PadButton, RawInput};
    use sapphire_platform::{GamepadType, LifecycleEvent, Scancode, TouchPhase};

    use super::*;

    fn ms(ms: u64) -> Timestamp {
        Timestamp::from_nanos(ms * 1_000_000)
    }

    fn input(desktop: bool) -> SandboxInput {
        let mut input =
            SandboxInput::new(desktop, Size { width: 1000, height: 500 }).expect("default bindings");
        input.update(ms(0));
        input
    }

    impl SandboxInput {
        fn push(&mut self, t: u64, raw: RawInput) {
            self.system.push(ms(t), raw);
        }

        fn key(&mut self, t: u64, scancode: Scancode, down: bool) {
            self.push(t, RawInput::Key { scancode, down });
        }

        fn orbit(&self) -> Vec2 {
            self.system.state(self.actions.orbit).value()
        }

        /// The distance factor this frame's zoom applies.
        fn zoom_factor(&self) -> f32 {
            self.system.state(self.actions.zoom).axis().exp()
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
        assert_eq!(input.take_actions(), SimActions { toggle_slow_motion: true, hitstop: true });
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
        input.system.release_all(ms(20));
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
        assert_eq!(input.take_actions(), SimActions { toggle_slow_motion: true, hitstop: false });
        touch(&mut input, 1000, 0, TouchPhase::Down, 300.0, 300.0);
        input.update(ms(1600));
        assert_eq!(input.take_actions(), SimActions { toggle_slow_motion: false, hitstop: true });
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
}
