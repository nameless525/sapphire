//! [`CombatControls`]: one `sapphire_input` system, read into a tick's input and a pause.

use std::time::Duration;

use sapphire_gameplay::{ActorActions, ActorId, Buttons, Stick, TickActions};
use sapphire_input::{
    ActionEventKind, ActionId, ActionKind, ContextId, InputBindings, InputMap, InputSystem, MapError,
};
use sapphire_math::Vec2;
use sapphire_platform::{Event, Timestamp};
use sapphire_time::{FixedStepDriver, FixedSteps};

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

/// An action's value as the simulation's stick. Keys add up past the stick's range, a diagonal's two
/// by √2, so the length is limited first. The simulation sees only the whole steps that come out.
fn stick_of(value: Vec2) -> Stick {
    let axes = value.clamp_length_max(1.0);
    Stick::from_axes(axes.x, axes.y)
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
///     sim.step(&controls.next_tick(player));
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
    /// The latest update's stick.
    stick: Stick,
    /// A stick that came and went since the last step: the next one reads it if it reads no other.
    stick_tap: Option<Stick>,
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
            stick: Stick::CENTER,
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
        self.stick = stick_of(movement.value());
        // Started and over inside this update, which keys and on-screen controls can do: no later
        // value will show it, so a step has to read it from here.
        if !movement.is_triggered()
            && let Some(value) = moved
        {
            self.stick_tap = Some(stick_of(value));
        }
    }

    /// What the next fixed step reads. Call it exactly once per step: each call moves a press along.
    #[must_use = "a press this returns is gone from the next call"]
    pub fn next_actions(&mut self) -> ActorActions {
        let tap = self.stick_tap.take();
        let stick = if self.stick == Stick::CENTER { tap.unwrap_or(Stick::CENTER) } else { self.stick };
        ActorActions { buttons: self.latch.next_step(), stick }
    }

    /// [`Self::next_actions`] as a tick for the simulation, for `player`, with everyone else neutral.
    /// Replays record what this returns.
    #[must_use = "a press this returns is gone from the next call"]
    pub fn next_tick(&mut self, player: ActorId) -> TickActions {
        let mut tick = TickActions::NEUTRAL;
        tick.set(player, self.next_actions()).expect("an empty tick has room for one actor");
        tick
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
