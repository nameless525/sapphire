//! [`CombatControls`]: one `sapphire_input` system, read into a tick's input and a pause.

use std::time::Duration;

use sapphire_gameplay::replay::ActionSource;
use sapphire_gameplay::{ActorActions, ActorId, Buttons, Stick, TickActions};
use sapphire_input::{
    ActionEventKind, ActionId, ActionKind, ContextId, InputBindings, InputMap, InputSystem, MapError,
};
use sapphire_math::{Vec2, Vec3, det};
use sapphire_platform::{Event, Timestamp};
use sapphire_time::{FixedStepDriver, FixedSteps};
use sapphire_ui::{Button, ButtonEventKind, TouchActions};

use crate::latch::PressLatch;

/// The default bindings document, compiled in: the app must stay controllable when content fails to
/// load, and they don't differ per cook.
pub const DEFAULT_BINDINGS: &str = include_str!("../combat_bindings.ron");

/// The context whose bindings play the game, enabled at [`COMBAT_PRIORITY`].
pub const COMBAT_CONTEXT: &str = "combat";
/// The context that replaces it while paused, enabled at [`PAUSE_PRIORITY`].
pub const PAUSE_CONTEXT: &str = "pause";
/// The combat context's priority. An app's own contexts go above it, and take what they bind from it.
pub const COMBAT_PRIORITY: i32 = 0;
/// The pause context's priority: above an app's own contexts, so it takes what it binds from them.
pub const PAUSE_PRIORITY: i32 = 100;

/// Each button of [`Buttons`] and the action that drives it. An on-screen control of the same name
/// drives it too ([`CombatControls::set_on_screen`]).
pub const BUTTON_ACTIONS: [(Buttons, &str); 8] = [
    (Buttons::LIGHT, "light"),
    (Buttons::HEAVY, "heavy"),
    (Buttons::DODGE, "dodge"),
    (Buttons::SKILL, "skill"),
    (Buttons::ULTIMATE, "ultimate"),
    (Buttons::SWAP, "swap"),
    (Buttons::LOCK_ON, "lock_on"),
    (Buttons::JUMP, "jump"),
];

/// The action that drives the stick, and the on-screen control that drives it too.
pub const MOVE_ACTION: &str = "move";

/// Each on-screen button and the verb it presses, through the
/// [`Source::Virtual`](sapphire_input::Source::Virtual) of that name. The attack's hold is the
/// button's own business: nothing in a tick follows it until a fighter has a move for it.
pub const TOUCH_BUTTONS: [(Button, &str); Button::ALL.len()] =
    [(Button::Attack, "light"), (Button::Dodge, "dodge"), (Button::Skill, "skill")];

const LOOK_ACTION: &str = "look";
const ZOOM_ACTION: &str = "camera_zoom";
const PAUSE_ACTION: &str = "pause";
const RESUME_ACTION: &str = "resume";
const LEAVE_ACTION: &str = "leave";

/// A bindings document combat can't run on.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ControlsError {
    /// The document doesn't compile. Its messages name the context and binding, or for a document
    /// that doesn't parse, the line and column.
    #[error(transparent)]
    Map(#[from] MapError),
    #[error("the bindings have no `{0}` action, which combat reads")]
    MissingAction(&'static str),
    #[error("action `{name}` is {found:?}, and combat reads it as {expected:?}")]
    WrongActionKind { name: &'static str, found: ActionKind, expected: ActionKind },
    #[error("the bindings have no `{0}` context, which combat enables")]
    MissingContext(&'static str),
}

/// The default bindings as a document, to rebind by editing or to add an app's own contexts to.
pub fn default_bindings() -> Result<InputBindings, MapError> {
    Ok(sapphire_serialize::from_ron(DEFAULT_BINDINGS)?)
}

/// How the mapping is doing (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ControlsStats {
    /// Presses of the combat verbs counted so far.
    pub presses: u64,
    /// Presses dropped because a button already had as many waiting as a step can catch up on: a
    /// long hitch under a mashing thumb, and nothing else, so anything above zero is worth a look.
    pub presses_over_limit: u64,
}

/// Every action the mapping reads, resolved.
#[derive(Clone, Copy, Debug)]
struct ActionIds {
    buttons: [ActionId; BUTTON_ACTIONS.len()],
    movement: ActionId,
    look: ActionId,
    zoom: ActionId,
    pause: ActionId,
    resume: ActionId,
    leave: ActionId,
}

impl ActionIds {
    fn new(map: &InputMap) -> Result<Self, ControlsError> {
        let of_kind = |name: &'static str, expected: ActionKind| {
            let id = map.action(name).ok_or(ControlsError::MissingAction(name))?;
            let found = map.action_kind(id);
            if found == expected {
                Ok(id)
            } else {
                Err(ControlsError::WrongActionKind { name, found, expected })
            }
        };
        let movement = of_kind(MOVE_ACTION, ActionKind::Axis2D)?;
        let mut buttons = [movement; BUTTON_ACTIONS.len()];
        for (id, (_, name)) in buttons.iter_mut().zip(BUTTON_ACTIONS) {
            *id = of_kind(name, ActionKind::Button)?;
        }
        Ok(Self {
            buttons,
            movement,
            look: of_kind(LOOK_ACTION, ActionKind::Axis2D)?,
            zoom: of_kind(ZOOM_ACTION, ActionKind::Axis1D)?,
            pause: of_kind(PAUSE_ACTION, ActionKind::Button)?,
            resume: of_kind(RESUME_ACTION, ActionKind::Button)?,
            leave: of_kind(LEAVE_ACTION, ActionKind::Button)?,
        })
    }
}

/// An action's value as a stick: keys add up past its range, a diagonal's two by √2, and a square
/// gate's corner reads (1, 1), so the length is limited to 1 before anything turns it.
fn raw_stick_of(value: Vec2) -> Vec2 {
    value.clamp_length_max(1.0)
}

/// A raw stick as the simulation's: turned from the view into the world, then quantized.
///
/// Everything faces +Z at yaw 0 (spec §5.4), and what's to the right of something facing +Z is −X in
/// a right-handed world, so the stick goes in as a thing facing +Z with its x turned round. A length
/// of at most 1 stays one through any turn, so no axis clips and the direction stays what it was.
/// `det::rotate_y` is pinned, so a table of yaws here reads the same on every OS.
fn world_stick(raw: Vec2, camera_yaw: f32) -> Stick {
    let world = det::rotate_y(Vec3::new(-raw.x, 0.0, raw.y), camera_yaw);
    Stick::from_axes(world.x, world.z)
}

/// Combat's controls: `sapphire_input`'s system, read once per frame and handed out once per fixed
/// step. The crate docs have what it promises; this is the frame:
///
/// ```no_run
/// # use std::time::Duration;
/// # use sapphire_controls::CombatControls;
/// # use sapphire_gameplay::{ActorId, Simulation};
/// # use sapphire_platform::{Event, Timestamp};
/// # use sapphire_time::FixedStepDriver;
/// # fn frame(
/// #     controls: &mut CombatControls,
/// #     driver: &mut FixedStepDriver,
/// #     sim: &mut Simulation,
/// #     player: ActorId,
/// #     camera_yaw: f32,
/// #     events: &[Event],
/// #     now: Timestamp,
/// #     frame_time: Duration,
/// # ) {
/// for event in events {
///     controls.handle_event(event);
/// }
/// controls.update(now);
/// if controls.wants_to_leave() {
///     return; // `AppControl::Exit`
/// }
/// let steps = controls.advance(driver, frame_time);
/// for _ in 0..steps.count {
///     sim.step(&controls.next_tick(player, camera_yaw));
/// }
/// // On the real clock, whatever the steps did.
/// let (look, zoom) = (controls.look(), controls.camera_zoom());
/// # let _ = (look, zoom);
/// # }
/// ```
#[derive(Debug)]
pub struct CombatControls {
    input: InputSystem,
    ids: ActionIds,
    combat: ContextId,
    pause: ContextId,
    latch: PressLatch,
    /// The latest update's stick, of length at most 1, as the player pushed it.
    stick: Vec2,
    /// A stick no step has read yet, which came and went or is still there: the next step reads it if
    /// it reads no other.
    stick_tap: Option<Vec2>,
    paused: bool,
    /// Resuming, once the combat context is back. It takes effect at the next update, and steps
    /// that ran before that would read a centred stick that's really tilted.
    resuming: bool,
    leaving: bool,
}

impl CombatControls {
    /// Controls on the default bindings.
    pub fn with_default_bindings() -> Result<Self, ControlsError> {
        Self::new(InputMap::new(default_bindings()?)?)
    }

    /// Controls on these bindings, with the combat context enabled. An app with contexts of its own
    /// compiles them into one document with [`default_bindings`]' and enables them through
    /// [`Self::input_mut`], at priorities between [`COMBAT_PRIORITY`] and [`PAUSE_PRIORITY`].
    pub fn new(map: InputMap) -> Result<Self, ControlsError> {
        let ids = ActionIds::new(&map)?;
        let context = |name| map.context(name).ok_or(ControlsError::MissingContext(name));
        let (combat, pause) = (context(COMBAT_CONTEXT)?, context(PAUSE_CONTEXT)?);
        let mut input = InputSystem::new(map);
        input.enable_context(combat, COMBAT_PRIORITY);
        Ok(Self {
            input,
            ids,
            combat,
            pause,
            latch: PressLatch::default(),
            stick: Vec2::ZERO,
            stick_tap: None,
            paused: false,
            resuming: false,
            leaving: false,
        })
    }

    /// The input system, for what isn't combat's: state, the last-used device, the app's own
    /// actions, [`InputSystem::rebind`] and [`InputSystem::capture_next`].
    pub fn input(&self) -> &InputSystem {
        &self.input
    }

    /// As [`Self::input`], to change it. Only [`Self::update`] may update it, or a frame's presses
    /// never reach the steps. A rebind keeps every action, so nothing here needs telling.
    pub fn input_mut(&mut self) -> &mut InputSystem {
        &mut self.input
    }

    /// Queues a platform event for the next update.
    pub fn handle_event(&mut self, event: &Event) {
        self.input.handle_event(event);
    }

    /// What on-screen controls read at `timestamp`: pressed buttons, and a stick with each axis
    /// within −1 to 1 and +y forward. Call it before [`Self::update`], whenever they change, more
    /// than once in a frame if a touch came and went. They reach the steps by the same road as a
    /// pad, under the same rules, each through the
    /// [`Source::Virtual`](sapphire_input::Source::Virtual) named as its action.
    pub fn set_on_screen(&mut self, timestamp: Timestamp, buttons: Buttons, stick: Vec2) {
        for (button, name) in BUTTON_ACTIONS {
            if let Some(control) = self.input.map().virtual_control(name) {
                let value = if buttons.contains(button) { Vec2::X } else { Vec2::ZERO };
                self.input.inject(control, value, timestamp);
            }
        }
        if let Some(control) = self.input.map().virtual_control(MOVE_ACTION) {
            self.input.inject(control, stick, timestamp);
        }
    }

    /// Takes what the touch zones read since their last update, before [`Self::update`], at `now`
    /// on the same clock: the same road as a pad, under the same rules (see the crate docs). Each
    /// button's edges go in at their own instants, so a tap that came and went inside a frame is
    /// still one press, and a thumb the OS took back fires nothing a release would. The stick and
    /// the camera's drag are this update's values. A finger that lands and lifts on the stick inside
    /// one update has no level to read, and moves nothing.
    pub fn read_touch(&mut self, touch: &TouchActions, now: Timestamp) {
        let input = &mut self.input;
        for event in &touch.events {
            let name =
                TOUCH_BUTTONS.iter().find(|(button, _)| *button == event.button).map(|(_, name)| *name);
            let Some(control) = name.and_then(|name| input.map().virtual_control(name)) else { continue };
            match event.kind {
                ButtonEventKind::Pressed => input.inject(control, Vec2::X, event.at),
                ButtonEventKind::Released => input.inject(control, Vec2::ZERO, event.at),
                ButtonEventKind::Canceled => input.cancel_virtual(control, event.at),
                ButtonEventKind::Held => {}
            }
        }
        if let Some(control) = input.map().virtual_control(MOVE_ACTION) {
            input.inject(control, touch.stick.value, now);
        }
        if let Some(control) = input.map().virtual_control(LOOK_ACTION) {
            input.inject(control, touch.camera_delta, now);
        }
    }

    /// Once per frame, on the platform's event clock: judges the frame's input and keeps what it
    /// found for the steps.
    pub fn update(&mut self, now: Timestamp) {
        profiling::scope!("controls::update");
        self.input.update(now);
        if self.resuming {
            self.resuming = false;
            self.paused = false;
        }

        // In time order, so a press after the pause in the same frame is the pause's, not combat's,
        // while one before it arrives on the first step after the resume, as if no time had passed.
        let mut cut = self.paused;
        let mut wanted = None;
        let mut moved = None;
        for event in self.input.events() {
            if event.kind != ActionEventKind::Triggered {
                continue;
            }
            if event.action == self.ids.pause {
                cut = true;
                wanted = Some(true);
            } else if event.action == self.ids.resume {
                wanted = Some(false);
            } else if event.action == self.ids.leave {
                self.leaving = true;
            } else if cut {
                continue;
            } else if event.action == self.ids.movement {
                moved = Some(event.value);
            } else if let Some(index) = self.ids.buttons.iter().position(|&id| id == event.action) {
                self.latch.press(index);
            }
        }
        if let Some(paused) = wanted {
            self.set_paused(paused);
        }

        let mut held = Buttons::NONE;
        for (id, (button, _)) in self.ids.buttons.iter().zip(BUTTON_ACTIONS) {
            if self.input.state(*id).is_triggered() {
                held = held | button;
            }
        }
        self.latch.sample(held);

        let movement = self.input.state(self.ids.movement);
        self.stick = raw_stick_of(movement.value());
        // No later value will show this one if it ends before the next step: it may have started and
        // ended inside this update, which keys and on-screen controls can do, or this frame may have
        // no step and the next let go. Held as a pause comes it hasn't ended, and the first step after
        // the pause reads the real stick.
        if movement.is_triggered() {
            self.stick_tap = (!self.paused).then_some(self.stick);
        } else if let Some(value) = moved {
            self.stick_tap = Some(raw_stick_of(value));
        }
    }

    /// What the next fixed step reads. Call it exactly once per step: each call moves a press along.
    ///
    /// The stick comes out world-space, turned by `camera_yaw`, so the simulation never reads a camera
    /// and a replay carries the direction the player meant. `camera_yaw` is where the view looks
    /// across the ground, as an actor's yaw: +Z at 0, turning toward +X
    /// ([`det::direction_from_yaw`]). An actor walking straight away from the camera has that yaw,
    /// and any camera's forward vector gives it through [`det::yaw_of_direction`]. A yaw that isn't
    /// finite reads as a centred stick, so a broken camera can't move anyone.
    #[must_use = "a press this returns is gone from the next call"]
    pub fn next_actions(&mut self, camera_yaw: f32) -> ActorActions {
        let tap = self.stick_tap.take();
        let held = world_stick(self.stick, camera_yaw);
        let stick = match tap {
            Some(tap) if held == Stick::CENTER => world_stick(tap, camera_yaw),
            _ => held,
        };
        ActorActions { buttons: self.latch.next_step(), stick }
    }

    /// [`Self::next_actions`] as a tick for the simulation, for `player`, with everyone else neutral.
    #[must_use = "a press this returns is gone from the next call"]
    pub fn next_tick(&mut self, player: ActorId, camera_yaw: f32) -> TickActions {
        let mut tick = TickActions::NEUTRAL;
        tick.set(player, self.next_actions(camera_yaw)).expect("an empty tick has room for one actor");
        tick
    }

    /// These controls as a recording's source, for steps under this camera: hand it to
    /// `sapphire_gameplay::replay::Recording::step`. Each step it's asked for moves a press along, as
    /// [`Self::next_actions`] does.
    pub fn source(&mut self, camera_yaw: f32) -> ControlsSource<'_> {
        ControlsSource { controls: self, camera_yaw }
    }

    /// The stick as the player pushed it, before any camera: length at most 1, +x right and +y
    /// forward, as of the latest update. It's for presentation, such as a camera that leans with it.
    /// The simulation's stick comes from [`Self::next_actions`], and [`Self::look`] is the right
    /// stick's.
    pub fn raw_stick(&self) -> Vec2 {
        self.stick
    }

    /// This frame's steps. While paused there are none and the driver never hears of the frame, so
    /// no time builds up to catch up on: a resumed frame steps as if the pause hadn't happened, and
    /// the alpha holds, so presentation holds still.
    pub fn advance(&self, driver: &mut FixedStepDriver, frame_time: Duration) -> FixedSteps {
        if self.paused {
            FixedSteps { count: 0, alpha: driver.alpha(), dropped: Duration::ZERO }
        } else {
            driver.advance(frame_time)
        }
    }

    /// Whether the steps are stopped.
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Stops the steps, or starts them again from the next update. The pause and resume actions do
    /// this themselves: this is for what isn't a binding, such as a pause menu's button.
    pub fn set_paused(&mut self, paused: bool) {
        if paused && (!self.paused || self.resuming) {
            self.paused = true;
            self.resuming = false;
            self.input.disable_context(self.combat);
            self.input.enable_context(self.pause, PAUSE_PRIORITY);
        } else if !paused && self.paused && !self.resuming {
            self.resuming = true;
            self.input.disable_context(self.pause);
            self.input.enable_context(self.combat, COMBAT_PRIORITY);
        }
    }

    /// Whether the player left from the pause. The app ends through `AppControl::Exit`, as anywhere
    /// else: on Android too, where finishing the activity instead races its one-second wait for the
    /// SDL thread (spec §5.18).
    pub fn wants_to_leave(&self) -> bool {
        self.leaving
    }

    /// The camera's (yaw, pitch) this frame, in radians: presentation, on the real clock, and never
    /// in the tick's input.
    pub fn look(&self) -> Vec2 {
        self.input.state(self.ids.look).value()
    }

    /// The natural log of the camera's distance factor this frame: below zero moves closer.
    pub fn camera_zoom(&self) -> f32 {
        self.input.state(self.ids.zoom).axis()
    }

    pub fn stats(&self) -> ControlsStats {
        ControlsStats { presses: self.latch.presses(), presses_over_limit: self.latch.over_limit() }
    }
}

/// A live player's bindings as an [`ActionSource`]: see [`CombatControls::source`].
#[derive(Debug)]
pub struct ControlsSource<'a> {
    controls: &'a mut CombatControls,
    camera_yaw: f32,
}

impl ActionSource for ControlsSource<'_> {
    /// The next step's input for the player, who is first in `cast`, with everyone else neutral. A
    /// cast with no one in it leaves no one to hand it to, and the step still moves presses along.
    fn actions(&mut self, _tick: u64, cast: &[ActorId]) -> TickActions {
        let actions = self.controls.next_actions(self.camera_yaw);
        let mut tick = TickActions::NEUTRAL;
        if let Some(&player) = cast.first() {
            tick.set(player, actions).expect("an empty tick has room for one actor");
        }
        tick
    }
}
