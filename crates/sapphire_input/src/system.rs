//! [`InputSystem`]: raw device input in, timestamped action events and action states out.

use std::time::Duration;

use sapphire_math::Vec2;
use sapphire_platform::{Event, GamepadId, Timestamp, TouchPhase};

use crate::bindings::{ActionKind, InputBindings};
use crate::devices::{DeviceKind, Devices, MAX_FINGERS};
use crate::gesture::{Gestures, PressChange};
use crate::map::{ActionId, Channel, ContextId, InputMap, MapError, SourceId, VirtualControl};
use crate::modifier;
use crate::raw::{PlatformAdapter, RawInput};
use crate::source::{Key, MouseButton, PadAxis, Source};
use crate::trigger::{self, Abort, Memory, TriggerSpec, TriggerState};

/// The longest update [`Modifier::ScaleByDeltaTime`](crate::Modifier::ScaleByDeltaTime) sees,
/// so a hitch or a return from the background doesn't fling a rate-driven value.
pub const MAX_UPDATE_DELTA: Duration = Duration::from_millis(100);

/// Raw inputs queued between updates before the queue has to grow.
const QUEUE_CAPACITY: usize = 256;
/// Action events in one update before the list has to grow.
const EVENT_CAPACITY: usize = 64;
/// Stick or trigger travel that a rebinding capture takes as a choice.
const CAPTURE_AXIS: f32 = 0.5;
/// The share of its actuation threshold under which an actuated binding lets go.
const LET_GO_SHARE: f32 = 0.9;

/// Where an action is after an update (Enhanced Input's trigger state).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ActionPhase {
    #[default]
    Idle,
    /// A binding is on its way to triggering: a hold not yet long enough, a tap in progress,
    /// a double tap waiting for its second press.
    Ongoing,
    Triggered,
}

/// What happened to an action (Enhanced Input's trigger events).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ActionEventKind {
    /// It left `Idle`.
    Started,
    /// It triggered: entered `Triggered`, or a pulse trigger (pressed, released, tap, double
    /// tap) fired.
    Triggered,
    /// It returned to `Idle` after triggering.
    Completed,
    /// It returned to `Idle` without triggering: a hold let go early, a tap held too long.
    Canceled,
}

/// What points at the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pointer {
    Mouse,
    /// A finger's slot, below [`MAX_FINGERS`].
    Finger(usize),
}

/// What a pointer did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerPhase {
    Down,
    Moved,
    Up,
    /// It's gone, and nothing it was doing should happen: the OS took back the finger, or focus was lost.
    Canceled,
}

const fn pointer_phase(phase: TouchPhase) -> PointerPhase {
    match phase {
        TouchPhase::Down => PointerPhase::Down,
        TouchPhase::Moved => PointerPhase::Moved,
        TouchPhase::Up => PointerPhase::Up,
        TouchPhase::Canceled => PointerPhase::Canceled,
    }
}

/// One thing a pointer did, for what depends on where it is: see [`InputSystem::pointers`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerEvent {
    pub pointer: Pointer,
    pub phase: PointerPhase,
    /// In points, +y down.
    pub position: Vec2,
    pub timestamp: Timestamp,
}

/// One change to an action, at the instant it happened.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActionEvent {
    pub action: ActionId,
    pub kind: ActionEventKind,
    /// The action's value: 1 in `x` for a triggered button, the bindings' sum for an axis,
    /// zero once completed or canceled.
    pub value: Vec2,
    /// On the platform's event clock, the same as [`sapphire_platform::Timestamp`]s: the
    /// timestamp of the device event that caused it, or for something time alone caused, the
    /// instant it happened (a hold completes at its press plus the hold time). Never earlier
    /// than the previous update, and in order within an update.
    pub timestamp: Timestamp,
}

/// An action as of the latest update.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ActionState {
    phase: ActionPhase,
    value: Vec2,
    fired: u32,
    since: Timestamp,
    triggered_since_start: bool,
    /// Latched by a [`Trigger::Toggle`](crate::Trigger::Toggle) binding's press, until the next.
    toggled: bool,
}

impl ActionState {
    pub fn phase(&self) -> ActionPhase {
        self.phase
    }

    /// Whether the action is held in `Triggered` (a held button, a moving stick).
    pub fn is_triggered(&self) -> bool {
        self.phase == ActionPhase::Triggered
    }

    /// Whether it triggered during the latest update, pulses included: "pressed this frame".
    pub fn fired(&self) -> bool {
        self.fired > 0
    }

    /// How many times it triggered during the latest update.
    pub fn fire_count(&self) -> u32 {
        self.fired
    }

    /// The value at the end of the update: the sum of the triggered bindings' values (1 in
    /// `x` for a triggered button).
    pub fn value(&self) -> Vec2 {
        self.value
    }

    /// The value's `x`: a 1D action's value.
    pub fn axis(&self) -> f32 {
        self.value.x
    }

    /// When the current phase began.
    pub fn since(&self) -> Timestamp {
        self.since
    }
}

/// Which devices a rebinding capture ([`InputSystem::capture_next`]) takes its input from. Input from any
/// other goes on to the bindings as usual, so a menu can still be backed out of while a pad's button is
/// awaited.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CaptureFrom {
    /// A key, a mouse button, a gamepad's button or a pushed axis.
    #[default]
    Anything,
    Keyboard,
    Mouse,
    /// A gamepad's button, or a stick or trigger pushed past half way.
    Gamepad,
}

impl CaptureFrom {
    fn takes(self, from: Self) -> bool {
        self == Self::Anything || self == from
    }
}

/// A rebind that couldn't be applied; the bindings are unchanged.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RebindError {
    #[error("no binding is named `{0}`")]
    UnknownBinding(String),
    #[error(transparent)]
    Map(#[from] MapError),
    #[error("the new bindings have no `{0}` context, which is enabled here")]
    LostContext(String),
}

#[derive(Clone, Copy, Debug)]
struct EnabledContext {
    id: ContextId,
    priority: i32,
    /// When it was last enabled: among equal priorities, the latest wins.
    order: u64,
}

/// A binding's latest result.
#[derive(Clone, Copy, Debug, Default)]
struct Output {
    state: TriggerState,
    /// Whether it fired an instantaneous trigger in the evaluation that produced it. A pulse is
    /// an event of that instant, not state, so a replayed output never carries it.
    pulse: bool,
    value: Vec2,
}

#[derive(Clone, Copy, Debug, Default)]
struct BindingState {
    memory: Memory,
    output: Output,
    /// A motion binding's share of its channel's motion this update.
    accepted: Accepted,
    active: bool,
    /// A captured analog source remains physical state, but cannot dispatch until it's at rest: its
    /// value zero once its modifiers have had it, so at or under its dead zone, whatever its threshold.
    captured: bool,
    /// Scratch for rebuilding the active set.
    include: bool,
    /// The chords that block it, a range of `InputSystem::blockers`.
    blockers: (usize, usize),
}

/// The motion a motion binding has taken this update: only what arrived while its gate was
/// open (its chord held, no chord blocking it), so a drag counts from its press to its release
/// however the frames fall. A gate changes only at an evaluation, so each evaluation settles
/// the motion that arrived since the one before under that one's gate (review FI-002).
#[derive(Clone, Copy, Debug, Default)]
struct Accepted {
    /// The update the sums below belong to.
    frame: u64,
    /// Whether the gate was open as of the latest evaluation.
    open: bool,
    /// The channel's sum when the gate last opened this update.
    opened_at: Vec2,
    /// What earlier open stretches of this update took.
    banked: Vec2,
}

impl Accepted {
    /// Settles the motion up to `reading` (the channel's sum in update `frame`), then sets the
    /// gate from here on. Returns what the binding has taken this update.
    ///
    /// While the gate stays open all update, the result is `reading` exactly.
    fn settle(&mut self, reading: Vec2, frame: u64, open: bool) -> Vec2 {
        if self.frame != frame {
            // The channel's sum starts again from zero, and the gate carries on.
            *self = Self { frame, open: self.open, ..Self::default() };
        }
        match (self.open, open) {
            (true, false) => self.banked += reading - self.opened_at,
            (false, true) => self.opened_at = reading,
            _ => {}
        }
        self.open = open;
        if open { self.banked + (reading - self.opened_at) } else { self.banked }
    }

    /// Whether the gate opened after motion had arrived this update and has let none through
    /// since, given what `settle` returned. The binding's first motion is then not its
    /// channel's first of the update, which is judged anyway, so it needs judging of its own.
    fn waiting(&self, accepted: Vec2) -> bool {
        self.open && self.opened_at != Vec2::ZERO && accepted == Vec2::ZERO
    }
}

/// Which motion bindings an evaluation judges afresh; the others keep their last state and
/// value, without its pulse. Every evaluation settles every motion binding's gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MotionPass {
    Keep,
    /// Advance only due motion timers, retaining their last judged actuation and value.
    Deadlines,
    Channel(Channel),
    All,
    /// The OS took a finger back: touch's motion bindings are judged and their runs end there, as
    /// under [`Abort::All`] (review WA-5). Other devices' motion bindings keep their last state,
    /// and every binding that isn't motion is judged under the evaluation's own abort.
    TouchCanceled,
}

impl MotionPass {
    fn judges(self, channel: Channel) -> bool {
        match self {
            Self::Keep | Self::Deadlines => false,
            Self::Channel(fresh) => channel == fresh,
            Self::All => true,
            Self::TouchCanceled => matches!(channel, Channel::Drag | Channel::Pinch),
        }
    }
}

/// The bindings' outputs for one action, combined.
#[derive(Clone, Copy, Debug, Default)]
struct Combined {
    state: TriggerState,
    value: Vec2,
    pulse: bool,
    pulse_value: Vec2,
    /// Whether any of its active bindings is a toggle, and how many toggles' presses this evaluation saw.
    toggles: bool,
    flips: u32,
}

impl Combined {
    fn add(&mut self, output: Output) {
        self.state = self.state.max(output.state);
        if output.state == TriggerState::Triggered {
            self.value = finite_sum(self.value, output.value);
        }
        if output.pulse {
            self.pulse = true;
            self.pulse_value = finite_sum(self.pulse_value, output.value);
        }
    }
}

/// Preserve the earlier accepted contribution if adding another would overflow.
fn finite_sum(held: Vec2, more: Vec2) -> Vec2 {
    let sum = held + more;
    if sum.is_finite() { sum } else { held }
}

/// The input pipeline: an [`InputMap`], the contexts enabled from it, and device state.
///
/// Each frame:
/// 1. Feed every platform event to [`handle_event`](Self::handle_event) (or raw input to
///    [`push`](Self::push)). Nothing is evaluated yet.
/// 2. Call [`update`](Self::update) once with the current time on the platform's event clock
///    (`Platform::now`). It plays the queued input back in time order, judging every binding at each
///    input's own timestamp, so a press and release within one frame both count and holds
///    complete at their exact instant. Motion is summed per update, each motion binding taking
///    only what arrives while its chord is held (see the crate docs).
/// 3. Read the [`events`](Self::events) of that update (for intents with their timestamps) and
///    [`state`](Self::state) (for held buttons and axes).
#[derive(Debug)]
pub struct InputSystem {
    map: InputMap,
    adapter: PlatformAdapter,
    devices: Devices,
    gestures: Gestures,
    queue: Vec<(Timestamp, RawInput)>,
    events: Vec<ActionEvent>,
    pointers: Vec<PointerEvent>,
    /// Where the mouse last was, whether its left button is down, and each finger that is.
    mouse_at: Vec2,
    mouse_down: bool,
    fingers_down: u32,
    contexts: Vec<EnabledContext>,
    next_context_order: u64,
    contexts_changed: bool,
    /// Active binding indices, grouped by action in evaluation order.
    active: Vec<usize>,
    /// Each action's range of `active`.
    action_ranges: Vec<(usize, usize)>,
    /// Chord actions blocking active bindings; see `BindingState::blockers`.
    blockers: Vec<ActionId>,
    /// Scratch for rebuilding the active set: sources claimed by consuming actions.
    claimed: Vec<SourceId>,
    bindings: Vec<BindingState>,
    actions: Vec<ActionState>,
    /// The latest evaluation's time.
    time: Timestamp,
    last_update: Option<Timestamp>,
    /// This update's length in seconds, for `ScaleByDeltaTime`.
    dt: f32,
    /// Counts updates; motion belongs to the update it arrived in.
    frame: u64,
    /// Channels with a motion binding whose gate opened after the update's first motion and
    /// has let nothing through since: their next motion is judged at once, so the binding's
    /// action starts at its timestamp.
    waiting: [bool; Channel::COUNT],
    next_deadline: Option<Timestamp>,
    capture_armed: Option<CaptureFrom>,
    captured: Option<Source>,
}

impl InputSystem {
    /// A system with no contexts enabled.
    pub fn new(map: InputMap) -> Self {
        let bindings = map.bindings().len();
        Self {
            adapter: PlatformAdapter::default(),
            devices: Devices::new(map.virtual_count()),
            gestures: Gestures::default(),
            queue: Vec::with_capacity(QUEUE_CAPACITY),
            events: Vec::with_capacity(EVENT_CAPACITY),
            pointers: Vec::with_capacity(EVENT_CAPACITY),
            mouse_at: Vec2::ZERO,
            mouse_down: false,
            fingers_down: 0,
            contexts: Vec::with_capacity(map.context_count()),
            next_context_order: 0,
            contexts_changed: true,
            active: Vec::with_capacity(bindings),
            action_ranges: vec![(0, 0); map.action_count()],
            blockers: Vec::with_capacity(map.blocker_capacity()),
            claimed: Vec::with_capacity(bindings),
            bindings: vec![BindingState::default(); bindings],
            actions: vec![ActionState::default(); map.action_count()],
            time: Timestamp::default(),
            last_update: None,
            dt: 0.0,
            frame: 0,
            waiting: [false; Channel::COUNT],
            next_deadline: None,
            capture_armed: None,
            captured: None,
            map,
        }
    }

    pub fn map(&self) -> &InputMap {
        &self.map
    }

    /// Enables `context` at `priority` (higher wins), or moves it there if it is already
    /// enabled. Among equal priorities the most recently enabled wins, so contexts stack.
    /// Takes effect at the next update; inputs already held when it does are ignored by its
    /// pressed, released, tap and hold bindings until they're let go.
    pub fn enable_context(&mut self, context: ContextId, priority: i32) {
        let order = self.next_context_order;
        self.next_context_order += 1;
        match self.contexts.iter_mut().find(|c| c.id == context) {
            Some(enabled) => *enabled = EnabledContext { id: context, priority, order },
            None => self.contexts.push(EnabledContext { id: context, priority, order }),
        }
        self.contexts_changed = true;
    }

    /// Disables `context` from the next update. Its actions complete or cancel then.
    pub fn disable_context(&mut self, context: ContextId) {
        self.contexts.retain(|c| c.id != context);
        self.contexts_changed = true;
    }

    pub fn is_enabled(&self, context: ContextId) -> bool {
        self.contexts.iter().any(|c| c.id == context)
    }

    /// The window's size in window coordinates. Resizes arriving through
    /// [`handle_event`](Self::handle_event) update it; set it once at startup.
    pub fn set_window_size(&mut self, width: f32, height: f32) {
        self.adapter.window_size = Vec2::new(width, height);
    }

    /// What a window coordinate is in points ([`Window::points_per_coordinate`]), which touches are
    /// measured in, so a tap's slop, a swipe's length and a drag's rate are the same size on any
    /// screen. 1 until told, as on iOS and a Mac; a window whose coordinates are pixels, as on Android,
    /// needs it from the start, and again if it moves to another screen. Reading it once a frame is
    /// cheap enough.
    ///
    /// [`Window::points_per_coordinate`]: sapphire_platform::Window::points_per_coordinate
    pub fn set_points_per_coordinate(&mut self, points: f32) {
        self.adapter.points_per_coordinate = points;
    }

    /// Queues a platform event for the next update. Events that aren't input are ignored,
    /// except that losing focus or the foreground releases everything held, firing nothing a
    /// release would. Key and mouse button releases queued right before the loss count as part
    /// of it, since the platform reports held keys that way, so a real release in that same
    /// frame, with no other input after it, fires nothing either.
    ///
    /// Touches over on-screen controls belong to the UI: give them to the control (which
    /// [`inject`](Self::inject)s), not here, or they would also drag the camera.
    pub fn handle_event(&mut self, event: &Event) {
        let mouse = match *event {
            Event::MouseMotion(motion) => Some((motion.timestamp, motion.x, motion.y)),
            Event::MouseButton(button) => Some((button.timestamp, button.x, button.y)),
            _ => None,
        };
        if let Some((timestamp, x, y)) = mouse {
            let position = Vec2::new(x, y) * self.adapter.points_per_coordinate;
            self.push(timestamp, RawInput::MouseAt { position });
        }
        let Some((timestamp, input)) = self.adapter.translate(event) else { return };
        if input == RawInput::ReleaseAll {
            // `SDL_SetKeyboardFocus(NULL)` posts an ordinary key-up for every held key, then the
            // loss, all in one call, so they reach this queue with no update between them (review
            // WA-1). Nothing marks those ups: their keyboard ID is also that of Windows' real key
            // events, and their raw code of 0 is also Cocoa's A. Wayland's pointer leave raises
            // held mouse buttons the same way, and may come just before the keyboard's leave.
            while let Some((
                _,
                RawInput::Key { down: false, .. }
                | RawInput::MouseButton { down: false, .. }
                | RawInput::MouseAt { .. },
            )) = self.queue.last()
            {
                self.queue.pop();
            }
        }
        self.push(timestamp, input);
    }

    /// Queues raw input for the next update. Nonfinite numeric payloads, and a finger slot at or past
    /// [`MAX_FINGERS`], are ignored. Motion, modifier and binding sums that overflow are ignored at the
    /// boundary that combines them, so action values stay finite even for synthetic input or extreme
    /// authored scales.
    pub fn push(&mut self, timestamp: Timestamp, input: RawInput) {
        let valid = match input {
            RawInput::MouseMotion { delta } | RawInput::MouseWheel { delta } => delta.is_finite(),
            RawInput::Touch { finger, .. } if finger >= MAX_FINGERS => false,
            RawInput::Touch { phase: TouchPhase::Canceled, .. } => true,
            RawInput::Touch { position, .. } => position.is_finite(),
            RawInput::Virtual { value, .. } => value.is_finite(),
            RawInput::PadAxis { value, .. } => value.is_finite(),
            _ => true,
        };
        if !valid {
            return;
        }
        self.queue.push((timestamp, input));
    }

    /// Sets an on-screen control's value (1 in `x` for a pressed button, a virtual stick's
    /// position) from the next update on. Bindings of [`Source::Virtual`] read it like any
    /// other device, through their modifiers and triggers.
    pub fn inject(&mut self, control: VirtualControl, value: Vec2, timestamp: Timestamp) {
        self.push(timestamp, RawInput::Virtual { control, value });
    }

    /// Lets an on-screen control go at `timestamp` because the OS took its finger back: it reads
    /// zero from the next update on, and nothing a release would fire fires. A
    /// [`Self::inject`] of zero is a player letting go, which a `Tap` binding takes for a tap.
    pub fn cancel_virtual(&mut self, control: VirtualControl, timestamp: Timestamp) {
        self.push(timestamp, RawInput::VirtualCanceled { control });
    }

    /// Releases everything held at `timestamp`, firing nothing a release would, and cancels
    /// sequences in progress: a double tap waiting for its second press goes idle then. As
    /// when [`handle_event`](Self::handle_event) sees focus go, the fingers down now are
    /// forgotten at once, so fingers landing later find slots even if the old ones' releases
    /// never come.
    pub fn release_all(&mut self, timestamp: Timestamp) {
        self.adapter.release_fingers();
        self.push(timestamp, RawInput::ReleaseAll);
    }

    /// Processes the queued input by its timestamps, whatever order it was pushed in, inputs at one
    /// instant as they were pushed. Then brings time-driven triggers up to `now`, a time on the
    /// platform's event clock no earlier than the queued input's.
    ///
    /// Allocates nothing, unless more input was queued, or more action events happened, than
    /// ever before (both lists keep their capacity).
    pub fn update(&mut self, now: Timestamp) {
        profiling::scope!("input::update");
        self.events.clear();
        self.pointers.clear();
        for action in &mut self.actions {
            action.fired = 0;
        }
        self.dt = self
            .last_update
            .map_or(0.0, |last| now.saturating_duration_since(last).min(MAX_UPDATE_DELTA).as_secs_f32());
        self.last_update = Some(now);
        if self.contexts_changed {
            self.contexts_changed = false;
            self.rebuild_active();
            self.evaluate(self.time, MotionPass::Keep, Abort::No);
        }

        // Taken out so `apply` can borrow the system; put back to keep its capacity.
        let mut queue = std::mem::take(&mut self.queue);
        sort_by_time(&mut queue);
        for &(timestamp, input) in &queue {
            let at = timestamp.max(self.time);
            self.run_deadlines(at);
            self.apply(at, input);
        }
        queue.clear();
        self.queue = queue;

        let at = now.max(self.time);
        self.run_deadlines(at);
        self.evaluate(at, MotionPass::All, Abort::No);
        self.frame += 1;
    }

    /// The action events of the latest update, in time order.
    pub fn events(&self) -> &[ActionEvent] {
        &self.events
    }

    /// What the mouse's left button and fingers did in the latest update, in time order, whatever the
    /// contexts and bindings: what's under a pointer decides what it means, and that's not known here.
    /// A screen's own touch controls take their fingers first, so those never get this far.
    pub fn pointers(&self) -> &[PointerEvent] {
        &self.pointers
    }

    pub fn state(&self, action: ActionId) -> &ActionState {
        &self.actions[action.index()]
    }

    /// The kind of device the player used last: for prompt glyphs.
    pub fn last_device(&self) -> Option<DeviceKind> {
        self.devices.last()
    }

    /// The gamepad in the player's hands: the device used last, if it was a pad. What rumble goes to, so
    /// a pad left on the desk stays still. `None` on the keyboard or on touch, which have no motors.
    pub fn active_gamepad(&self) -> Option<GamepadId> {
        self.adapter.pad(self.devices.active_pad()?)
    }

    /// Gamepads connected now. Connected says little of who's playing, since a simulator can report
    /// one nobody holds: on-screen controls follow [`Self::active_gamepad`] (spec §6.6).
    pub fn gamepads(&self) -> usize {
        self.devices.gamepads()
    }

    /// Makes the next key, mouse button, gamepad button or pushed axis from `from` a capture for
    /// rebinding instead of input: [`take_captured`](Self::take_captured) returns it. Only a press is
    /// taken, so the release of whatever armed it still reaches its bindings.
    pub fn capture_next(&mut self, from: CaptureFrom) {
        self.capture_armed = Some(from);
        self.captured = None;
    }

    /// Whether a capture is armed and hasn't taken anything yet.
    pub fn is_capturing(&self) -> bool {
        self.capture_armed.is_some()
    }

    /// Disarms a capture that hasn't taken anything, so the next press is input again.
    pub fn cancel_capture(&mut self) {
        self.capture_armed = None;
    }

    /// The source captured since [`capture_next`](Self::capture_next), once there is one.
    pub fn take_captured(&mut self) -> Option<Source> {
        self.captured.take()
    }

    /// Makes the binding named `name` read `source`, recompiling the map, and returns the
    /// source it read before. Enabled contexts and action phases carry over; every binding
    /// starts afresh, as on entering a context: edge triggers ignore anything held until it's
    /// released, and `Down` reads it at once, as a level. An on-screen control is no different:
    /// it keeps what it reads, and its [`VirtualControl`] keeps its meaning, as does input
    /// already queued for it.
    pub fn rebind(&mut self, name: &str, source: Source) -> Result<Source, RebindError> {
        let (document, previous) =
            self.map.rebound(name, source).ok_or_else(|| RebindError::UnknownBinding(name.to_owned()))?;
        self.replace_map(self.map.recompiled(document)?);
        Ok(previous)
    }

    /// Makes `map` what it reads, for an edited document of bindings. Every enabled context stays enabled at
    /// its priority, under its name in `map`, or this refuses and nothing changes. Actions and on-screen
    /// controls may have new places in `map`, so everything starts afresh, as on entering a context: edge
    /// triggers ignore what's held until it's released, `Down` reads it at once, and whatever an update
    /// hasn't played that names an on-screen control goes.
    pub fn replace_bindings(&mut self, map: InputMap) -> Result<(), RebindError> {
        let mut contexts = self.contexts.clone();
        for enabled in &mut contexts {
            let name = self.map.context_name(enabled.id);
            enabled.id = map.context(name).ok_or_else(|| RebindError::LostContext(name.to_owned()))?;
        }
        self.contexts = contexts;
        self.queue.retain(|(_, input)| {
            !matches!(input, RawInput::Virtual { .. } | RawInput::VirtualCanceled { .. })
        });
        self.events.clear();
        self.actions = vec![ActionState::default(); map.action_count()];
        self.action_ranges = vec![(0, 0); map.action_count()];
        self.devices.reset_virtuals(map.virtual_count());
        self.replace_map(map);
        Ok(())
    }

    /// The bindings as they stand, rebinding included: what user settings persist (M3).
    pub fn bindings(&self) -> &InputBindings {
        self.map.document()
    }

    fn replace_map(&mut self, map: InputMap) {
        let bindings = map.bindings().len();
        self.bindings = vec![BindingState::default(); bindings];
        self.active = Vec::with_capacity(bindings);
        self.claimed = Vec::with_capacity(bindings);
        self.blockers = Vec::with_capacity(map.blocker_capacity());
        self.devices.grow_virtuals(map.virtual_count());
        self.map = map;
        self.contexts_changed = true;
    }

    /// Captures `source`, which came from `from`, if a capture for it is armed; returns whether it did.
    fn capture(&mut self, from: CaptureFrom, source: impl FnOnce() -> Source) -> bool {
        if !self.capture_armed.is_some_and(|armed| armed.takes(from)) {
            return false;
        }
        self.capture_armed = None;
        self.captured = Some(source());
        true
    }

    fn apply(&mut self, now: Timestamp, input: RawInput) {
        match input {
            RawInput::Key { scancode, down } => {
                if down && self.capture(CaptureFrom::Keyboard, || Source::Key(Key::from_scancode(scancode))) {
                    return;
                }
                if down {
                    self.devices.used(DeviceKind::KeyboardMouse);
                }
                if self.devices.set_key(scancode, down) {
                    self.evaluate(now, MotionPass::Keep, Abort::No);
                }
            }
            RawInput::MouseButton { button, down } => {
                if down && self.capture(CaptureFrom::Mouse, || Source::Mouse(button)) {
                    return;
                }
                if down {
                    self.devices.used(DeviceKind::KeyboardMouse);
                }
                if button == MouseButton::Left && self.mouse_down != down {
                    self.mouse_down = down;
                    let phase = if down { PointerPhase::Down } else { PointerPhase::Up };
                    self.pointer(Pointer::Mouse, phase, self.mouse_at, now);
                }
                if self.devices.set_mouse(button, down) {
                    self.evaluate(now, MotionPass::Keep, Abort::No);
                }
            }
            RawInput::MouseMotion { delta } => {
                if delta != Vec2::ZERO {
                    self.devices.used(DeviceKind::KeyboardMouse);
                }
                self.add_motion(Channel::Pointer, delta, now);
            }
            RawInput::MouseAt { position } => {
                if position != self.mouse_at {
                    self.mouse_at = position;
                    self.pointer(Pointer::Mouse, PointerPhase::Moved, position, now);
                }
            }
            RawInput::MouseWheel { delta } => {
                self.devices.used(DeviceKind::KeyboardMouse);
                self.add_motion(Channel::Wheel, delta, now);
            }
            RawInput::GamepadAdded { pad, kind } => self.devices.connect_pad(pad, kind),
            RawInput::GamepadRemoved { pad } => {
                if self.devices.disconnect_pad(pad) {
                    self.evaluate(now, MotionPass::Keep, Abort::Release);
                }
            }
            RawInput::PadButton { pad, button, down } => {
                if down && self.capture(CaptureFrom::Gamepad, || Source::Pad(button)) {
                    return;
                }
                if self.devices.set_pad_button(pad, button, down) {
                    self.evaluate(now, MotionPass::Keep, Abort::No);
                }
            }
            RawInput::PadAxis { pad, axis, value } => {
                let held = self.devices.take_held_through_loss(pad, axis);
                let captured = !held
                    && value.abs() >= CAPTURE_AXIS
                    && self.capture(CaptureFrom::Gamepad, || Source::PadAxis(axis));
                let changed = self.devices.set_pad_axis(pad, axis, value);
                if changed || captured {
                    if captured {
                        for &index in &self.active {
                            let reads = self.map.bindings()[index].source.reads_axis(axis);
                            self.bindings[index].captured |= reads;
                        }
                    }
                    if held {
                        self.enter_held_axis(axis);
                    }
                    self.evaluate(now, MotionPass::Keep, Abort::No);
                }
            }
            RawInput::Touch { finger, phase, position } => {
                if phase == TouchPhase::Down {
                    self.devices.used(DeviceKind::Touch);
                }
                let (phase_now, bit) = (pointer_phase(phase), 1 << finger);
                match phase_now {
                    PointerPhase::Down => self.fingers_down |= bit,
                    PointerPhase::Up | PointerPhase::Canceled => self.fingers_down &= !bit,
                    PointerPhase::Moved => {}
                }
                self.pointer(Pointer::Finger(finger), phase_now, position, now);
                let out = self.gestures.touch(now, finger, phase, position, self.map.gestures());
                // An up can carry the last move: settle it under the press's existing chord gate.
                // Slop cancellation and OS cancellation keep their existing gate-first policy.
                if out.press == Some(PressChange::Up) {
                    if let Some(drag) = out.drag {
                        self.add_motion(Channel::Drag, drag, now);
                    }
                    if let Some(pinch) = out.pinch {
                        self.add_motion(Channel::Pinch, Vec2::new(pinch, 0.0), now);
                    }
                }
                if let Some(change) = out.press
                    && self.devices.set_press(change == PressChange::Down)
                {
                    let abort = if change == PressChange::Cancel { Abort::Release } else { Abort::No };
                    self.evaluate(now, MotionPass::Keep, abort);
                }
                if out.canceled {
                    self.evaluate(now, MotionPass::TouchCanceled, Abort::Release);
                }
                if out.press != Some(PressChange::Up) {
                    if let Some(drag) = out.drag {
                        self.add_motion(Channel::Drag, drag, now);
                    }
                    if let Some(pinch) = out.pinch {
                        self.add_motion(Channel::Pinch, Vec2::new(pinch, 0.0), now);
                    }
                }
                if let Some(direction) = out.swipe {
                    // A swipe is an instant: it presses and releases at the same timestamp.
                    self.devices.set_swipe(direction, true);
                    self.evaluate(now, MotionPass::Keep, Abort::No);
                    self.devices.set_swipe(direction, false);
                    self.evaluate(now, MotionPass::Keep, Abort::No);
                }
            }
            RawInput::Virtual { control, value } => {
                // An on-screen control in use is a finger on the screen, whatever else is connected.
                if value != Vec2::ZERO {
                    self.devices.used(DeviceKind::Touch);
                }
                if self.devices.set_virtual(control.index(), value) {
                    self.evaluate(now, MotionPass::Keep, Abort::No);
                }
            }
            RawInput::VirtualCanceled { control } => {
                if self.devices.set_virtual(control.index(), Vec2::ZERO) {
                    self.evaluate(now, MotionPass::Keep, Abort::Release);
                }
            }
            RawInput::ReleaseAll => {
                if std::mem::take(&mut self.mouse_down) {
                    self.pointer(Pointer::Mouse, PointerPhase::Canceled, self.mouse_at, now);
                }
                for finger in 0..MAX_FINGERS {
                    if self.fingers_down & (1 << finger) != 0 {
                        // Gone, so wherever it was doesn't matter.
                        self.pointer(Pointer::Finger(finger), PointerPhase::Canceled, Vec2::ZERO, now);
                    }
                }
                self.fingers_down = 0;
                self.devices.release_all();
                self.gestures.reset();
                // Motion bindings are judged too, on what they took so far, so the loss ends
                // their sequences and their runs as well (`Abort::All`).
                self.evaluate(now, MotionPass::All, Abort::All);
            }
        }
    }

    fn pointer(&mut self, pointer: Pointer, phase: PointerPhase, position: Vec2, timestamp: Timestamp) {
        self.pointers.push(PointerEvent { pointer, phase, position, timestamp });
    }

    /// `axis` has reported for the first time since everything let go, which it was held through.
    /// Its bindings take that as a binding becoming active takes what's held: edge triggers wait for it
    /// to let go, and a level reads on. One that another pad keeps actuated has no edge to lose.
    fn enter_held_axis(&mut self, axis: PadAxis) {
        for &index in &self.active {
            let binding = &self.map.bindings()[index];
            let reads = binding.source.reads_axis(axis);
            if reads && !self.bindings[index].memory.actuated() {
                self.bindings[index].memory = entering(&self.map, &self.devices, index, self.frame, self.dt);
            }
        }
    }

    /// Adds motion. The first of an update, and the first a binding takes once its gate
    /// opens, is judged at once, so the action starts at that motion's timestamp. The rest only
    /// add up until the update's final evaluation.
    fn add_motion(&mut self, channel: Channel, delta: Vec2, now: Timestamp) {
        if !delta.is_finite() {
            return;
        }
        let first = self.devices.add_motion(channel, delta, self.frame);
        if first || self.waiting[channel.index()] {
            self.evaluate(now, MotionPass::Channel(channel), Abort::No);
        }
    }

    /// Evaluates at every trigger deadline up to `until`, in order, so time-driven changes
    /// land at their exact instants and before later input.
    fn run_deadlines(&mut self, until: Timestamp) {
        // Each evaluation retires the deadlines it reaches and input alone creates new ones,
        // so this settles within one round per active binding.
        let mut rounds = self.active.len() + 1;
        while let Some(deadline) = self.next_deadline.filter(|&d| d <= until) {
            if rounds == 0 {
                debug_assert!(false, "trigger deadlines did not settle");
                return;
            }
            rounds -= 1;
            self.evaluate(deadline.max(self.time), MotionPass::Deadlines, Abort::No);
        }
    }

    /// Judges every active binding at `now` and turns the changes into action events.
    fn evaluate(&mut self, now: Timestamp, pass: MotionPass, abort: Abort) {
        self.time = now;
        self.waiting = [false; Channel::COUNT];
        let map = &self.map;
        for &action in map.order() {
            let (start, end) = self.action_ranges[action.index()];
            let mut combined = Combined::default();
            for &index in &self.active[start..end] {
                let binding = &map.bindings()[index];
                let state = &mut self.bindings[index];
                combined.toggles |= binding.trigger == TriggerSpec::Toggle;
                // Chords come earlier in the order, so they are already judged as of `now`.
                let triggered = |chord: ActionId| self.actions[chord.index()].phase == ActionPhase::Triggered;
                let gated = binding.chord.is_some_and(|chord| !triggered(chord))
                    || self.blockers[state.blockers.0..state.blockers.1]
                        .iter()
                        .any(|&chord| triggered(chord));
                let channel = binding.source.channel();
                let raw = match channel {
                    None => self.devices.value(binding.source, self.frame),
                    Some(channel) => {
                        let reading = self.devices.value(binding.source, self.frame);
                        let accepted = state.accepted.settle(reading, self.frame, !gated);
                        if state.accepted.waiting(accepted) {
                            self.waiting[channel.index()] = true;
                        }
                        if pass == MotionPass::Deadlines
                            && trigger::deadline(binding.trigger, &state.memory).is_some_and(|at| at <= now)
                        {
                            let actuated = state.memory.actuated();
                            let fire =
                                trigger::evaluate(binding.trigger, &mut state.memory, actuated, now, abort);
                            state.output.state = fire.state;
                            state.output.pulse = fire.pulse;
                            combined.add(state.output);
                            continue;
                        }
                        if !pass.judges(channel) {
                            // The pulse fired in the evaluation that produced it; replaying it
                            // would fire again for every unrelated input (review FI-006).
                            combined.add(Output { pulse: false, ..state.output });
                            continue;
                        }
                        accepted
                    }
                };
                let value = modifier::apply_all(map.modifiers(binding), raw, self.dt);
                let actuated = is_actuated(value, binding.threshold, state.memory.actuated());
                if state.captured {
                    // Not let go under its own threshold: a pull still on its way up would carry on past it.
                    state.captured = value != Vec2::ZERO;
                    state.memory = Memory::entering(actuated);
                    state.output = Output::default();
                    combined.add(state.output);
                    continue;
                }
                // Motion can't let go, so a cancel ends its run. In this pass the only motion to get
                // here is touch's, and another device's held key or armed double tap keeps `abort`.
                let abort =
                    if pass == MotionPass::TouchCanceled && channel.is_some() { Abort::All } else { abort };
                let fire = trigger::evaluate(binding.trigger, &mut state.memory, actuated, now, abort);
                // Motion met its gate as it arrived: what got through counts after the gate closes.
                state.output = if gated && channel.is_none() {
                    Output::default()
                } else {
                    combined.flips += u32::from(fire.flip);
                    Output { state: fire.state, pulse: fire.pulse, value }
                };
                combined.add(state.output);
            }
            let kind = map.actions()[action.index()].kind;
            let held = &mut self.actions[action.index()];
            // The latch lets go as a held input would: with everything, or with the last of its toggles.
            held.toggled =
                combined.toggles && abort != Abort::All && (held.toggled ^ (combined.flips % 2 == 1));
            if held.toggled {
                combined.state = TriggerState::Triggered;
            }
            transition(held, action, kind, combined, now, &mut self.events);
        }
        self.next_deadline = self
            .active
            .iter()
            .filter_map(|&index| {
                trigger::deadline(map.bindings()[index].trigger, &self.bindings[index].memory)
            })
            .min();
    }

    /// Recomputes which bindings are active after the enabled contexts changed: contexts in
    /// priority order, each losing the sources that higher contexts' consuming actions claimed.
    fn rebuild_active(&mut self) {
        let map = &self.map;
        self.contexts.sort_unstable_by(|a, b| b.priority.cmp(&a.priority).then(b.order.cmp(&a.order)));
        for state in &mut self.bindings {
            state.include = false;
        }
        self.claimed.clear();
        for enabled in &self.contexts {
            let range = map.contexts()[enabled.id.index()].bindings.clone();
            for index in range.clone() {
                self.bindings[index].include = !self.claimed.contains(&map.bindings()[index].source);
            }
            for index in range {
                let binding = &map.bindings()[index];
                if self.bindings[index].include
                    && map.actions()[binding.action.index()].consume
                    && !self.claimed.contains(&binding.source)
                {
                    self.claimed.push(binding.source);
                }
            }
        }

        self.active.clear();
        for &action in map.order() {
            let start = self.active.len();
            self.active.extend(
                (0..map.bindings().len())
                    .filter(|&index| self.bindings[index].include && map.bindings()[index].action == action),
            );
            self.action_ranges[action.index()] = (start, self.active.len());
        }

        // A chord blocks the unchorded bindings of its source in its own and lower contexts.
        let rank =
            |context: ContextId| self.contexts.iter().position(|c| c.id == context).unwrap_or(usize::MAX);
        self.blockers.clear();
        for &index in &self.active {
            let blocked = &map.bindings()[index];
            let start = self.blockers.len();
            if blocked.chord.is_none() {
                for &other in &self.active {
                    let chorded = &map.bindings()[other];
                    if let Some(chord) = chorded.chord
                        && chorded.source == blocked.source
                        && rank(chorded.context) <= rank(blocked.context)
                    {
                        self.blockers.push(chord);
                    }
                }
            }
            self.bindings[index].blockers = (start, self.blockers.len());
        }

        for (index, state) in self.bindings.iter_mut().enumerate() {
            match (state.active, state.include) {
                (false, true) => {
                    state.memory = entering(map, &self.devices, index, self.frame, self.dt);
                    state.output = Output::default();
                    // Closed until the evaluation after this rebuild opens it.
                    state.accepted = Accepted::default();
                }
                (true, false) => {
                    state.memory = Memory::default();
                    state.output = Output::default();
                    state.accepted = Accepted::default();
                }
                _ => {}
            }
            state.active = state.include;
        }
    }
}

/// Puts the queue in time order, inputs at one instant as they were pushed. On-screen controls come by
/// another road from the pump's events and are pushed after them, so a thumb's press can follow a Back
/// that came later. Those few inputs move past what they're behind and the rest stay put, so this is
/// nearly a pass over the queue, in place: std's stable sort allocates for a long queue.
fn sort_by_time(queue: &mut [(Timestamp, RawInput)]) {
    for next in 1..queue.len() {
        let mut at = next;
        while at > 0 && queue[at - 1].0 > queue[at].0 {
            queue.swap(at - 1, at);
            at -= 1;
        }
    }
}

/// What a binding remembers on becoming active with its source reading as it does now.
fn entering(map: &InputMap, devices: &Devices, index: usize, frame: u64, dt: f32) -> Memory {
    let binding = &map.bindings()[index];
    let raw = devices.value(binding.source, frame);
    let value = modifier::apply_all(map.modifiers(binding), raw, dt);
    Memory::entering(is_actuated(value, binding.threshold, false))
}

/// Whether a binding reading `value` is actuated, given whether it was: it takes `threshold` to start,
/// and falling a little under that to stop, so an analog trigger's noise at the threshold is one press.
fn is_actuated(value: Vec2, threshold: f32, was: bool) -> bool {
    let magnitude = value.length();
    magnitude > 0.0 && magnitude >= if was { threshold * LET_GO_SHARE } else { threshold }
}

fn phase(state: TriggerState) -> ActionPhase {
    match state {
        TriggerState::Idle => ActionPhase::Idle,
        TriggerState::Ongoing => ActionPhase::Ongoing,
        TriggerState::Triggered => ActionPhase::Triggered,
    }
}

/// An action's value in its own shape.
fn shaped(kind: ActionKind, value: Vec2, triggered: bool) -> Vec2 {
    match kind {
        ActionKind::Button => {
            if triggered {
                Vec2::X
            } else {
                Vec2::ZERO
            }
        }
        ActionKind::Axis1D => Vec2::new(value.x, 0.0),
        ActionKind::Axis2D => value,
    }
}

/// Moves an action to its bindings' combined state, emitting the events on the way.
fn transition(
    state: &mut ActionState,
    action: ActionId,
    kind: ActionKind,
    combined: Combined,
    now: Timestamp,
    events: &mut Vec<ActionEvent>,
) {
    use ActionEventKind::{Canceled, Completed, Started, Triggered};

    let previous = state.phase;
    let next = phase(combined.state);
    let value = shaped(kind, combined.value, next == ActionPhase::Triggered);
    let mut emit = |kind, value| events.push(ActionEvent { action, kind, value, timestamp: now });
    if combined.pulse {
        if previous == ActionPhase::Idle {
            emit(Started, value);
        }
        emit(Triggered, shaped(kind, finite_sum(combined.value, combined.pulse_value), true));
        state.fired += 1;
        state.triggered_since_start = true;
        if next == ActionPhase::Idle {
            emit(Completed, Vec2::ZERO);
        }
        // An idle action that pulsed has been idle since the pulse; a held one stays held.
        if next != previous || next == ActionPhase::Idle {
            state.since = now;
        }
    } else {
        match (previous, next) {
            (ActionPhase::Idle, ActionPhase::Ongoing) => {
                emit(Started, value);
                state.triggered_since_start = false;
            }
            (ActionPhase::Idle, ActionPhase::Triggered) => {
                emit(Started, value);
                emit(Triggered, value);
                state.fired += 1;
                state.triggered_since_start = true;
            }
            (ActionPhase::Ongoing, ActionPhase::Triggered) => {
                emit(Triggered, value);
                state.fired += 1;
                state.triggered_since_start = true;
            }
            (ActionPhase::Ongoing | ActionPhase::Triggered, ActionPhase::Idle) => {
                emit(if state.triggered_since_start { Completed } else { Canceled }, Vec2::ZERO);
            }
            _ => {}
        }
        if next != previous {
            state.since = now;
        }
    }
    state.phase = next;
    state.value = value;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_motion_follows_the_gate() {
        let v = |x: f32| Vec2::new(x, 0.0);
        // Open all update: exactly the channel's sum, however many evaluations read it.
        let mut accepted = Accepted::default();
        assert_eq!(accepted.settle(v(0.0), 1, true), v(0.0));
        assert_eq!(accepted.settle(v(0.1), 1, true), v(0.1));
        assert_eq!(accepted.settle(v(0.1 + 0.2 + 0.3), 1, true), v(0.1 + 0.2 + 0.3));

        // Closed at a reading of 1 and opened again at 5: what arrived in between doesn't count.
        let mut accepted = Accepted::default();
        accepted.settle(v(0.0), 1, true);
        assert_eq!(accepted.settle(v(1.0), 1, false), v(1.0), "arrived while open");
        assert_eq!(accepted.settle(v(5.0), 1, true), v(1.0), "arrived while closed");
        assert_eq!(accepted.settle(v(7.0), 1, true), v(3.0));
        assert_eq!(accepted.settle(v(7.0), 1, false), v(3.0));

        // A new update starts from nothing, under the gate as it was.
        assert_eq!(accepted.settle(v(2.0), 2, false), v(0.0), "closed since the last update");
        assert_eq!(accepted.settle(v(4.0), 2, true), v(0.0));
        assert_eq!(accepted.settle(v(6.0), 3, true), v(6.0), "open since the last update");
    }

    #[test]
    fn a_gate_opened_after_motion_waits_for_its_own() {
        let v = |x: f32| Vec2::new(x, 0.0);
        let mut accepted = Accepted::default();
        let taken = accepted.settle(v(0.0), 1, true);
        assert!(!accepted.waiting(taken), "open before any motion: the update's first is judged anyway");

        let mut accepted = Accepted::default();
        accepted.settle(v(0.0), 1, false);
        let taken = accepted.settle(v(2.0), 1, true);
        assert!(accepted.waiting(taken), "opened after motion: the next is the binding's first");
        let taken = accepted.settle(v(5.0), 1, true);
        assert!(!accepted.waiting(taken));
    }
}
