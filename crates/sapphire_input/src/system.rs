//! [`InputSystem`]: raw device input in, timestamped action events and action states out.

use std::time::Duration;

use sapphire_math::Vec2;
use sapphire_platform::{Event, Timestamp, TouchPhase};

use crate::bindings::{ActionKind, InputBindings};
use crate::devices::{DeviceKind, Devices};
use crate::gesture::{Gestures, PressChange};
use crate::map::{ActionId, Channel, ContextId, InputMap, MapError, SourceId, VirtualControl};
use crate::modifier;
use crate::raw::{PlatformAdapter, RawInput};
use crate::source::{Key, Source};
use crate::trigger::{self, Abort, Memory, TriggerState};

/// The longest update [`Modifier::ScaleByDeltaTime`](crate::Modifier::ScaleByDeltaTime) sees,
/// so a hitch or a return from the background doesn't fling a rate-driven value.
pub const MAX_UPDATE_DELTA: Duration = Duration::from_millis(100);

/// Raw inputs queued between updates before the queue has to grow.
const QUEUE_CAPACITY: usize = 256;
/// Action events in one update before the list has to grow.
const EVENT_CAPACITY: usize = 64;
/// Stick or trigger travel that a rebinding capture takes as a choice.
const CAPTURE_AXIS: f32 = 0.5;

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

/// A rebind that couldn't be applied; the bindings are unchanged.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RebindError {
    #[error("no binding is named `{0}`")]
    UnknownBinding(String),
    #[error(transparent)]
    Map(#[from] MapError),
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
    pulse: bool,
    value: Vec2,
}

#[derive(Clone, Copy, Debug, Default)]
struct BindingState {
    memory: Memory,
    output: Output,
    active: bool,
    /// Scratch for rebuilding the active set.
    include: bool,
    /// The chords that block it, a range of `InputSystem::blockers`.
    blockers: (usize, usize),
}

/// Which motion bindings an evaluation reads afresh; the others keep their last output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MotionPass {
    Keep,
    Channel(Channel),
    All,
}

impl MotionPass {
    fn reads(self, source: SourceId) -> bool {
        match (source.channel(), self) {
            (None, _) | (Some(_), Self::All) => true,
            (Some(channel), Self::Channel(fresh)) => channel == fresh,
            (Some(_), Self::Keep) => false,
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
}

impl Combined {
    fn add(&mut self, output: Output) {
        self.state = self.state.max(output.state);
        if output.state == TriggerState::Triggered {
            self.value += output.value;
        }
        if output.pulse {
            self.pulse = true;
            self.pulse_value += output.value;
        }
    }
}

/// The input pipeline: an [`InputMap`], the contexts enabled from it, and device state.
///
/// Each frame:
/// 1. Feed every platform event to [`handle_event`](Self::handle_event) (or raw input to
///    [`push`](Self::push)). Nothing is evaluated yet.
/// 2. Call [`update`](Self::update) once with the current time on the platform's event clock
///    (`Platform::now`). It plays the queued input back in order, judging every binding at each
///    input's own timestamp, so a press and release within one frame both count and holds
///    complete at their exact instant.
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
    next_deadline: Option<Timestamp>,
    capture_armed: bool,
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
            next_deadline: None,
            capture_armed: false,
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

    /// The window's size in window points, which touches are measured in. Resizes arriving
    /// through [`handle_event`](Self::handle_event) update it; set it once at startup.
    pub fn set_window_size(&mut self, width: f32, height: f32) {
        self.adapter.window_size = Vec2::new(width, height);
    }

    /// Queues a platform event for the next update. Events that aren't input are ignored,
    /// except that losing focus or the foreground releases everything held.
    ///
    /// Touches over on-screen controls belong to the UI: give them to the control (which
    /// [`inject`](Self::inject)s), not here, or they would also drag the camera.
    pub fn handle_event(&mut self, event: &Event) {
        if let Some((timestamp, input)) = self.adapter.translate(event) {
            self.push(timestamp, input);
        }
    }

    /// Queues raw input for the next update.
    pub fn push(&mut self, timestamp: Timestamp, input: RawInput) {
        self.queue.push((timestamp, input));
    }

    /// Sets an on-screen control's value (1 in `x` for a pressed button, a virtual stick's
    /// position) from the next update on. Bindings of [`Source::Virtual`] read it like any
    /// other device, through their modifiers and triggers.
    pub fn inject(&mut self, control: VirtualControl, value: Vec2, timestamp: Timestamp) {
        self.push(timestamp, RawInput::Virtual { control, value });
    }

    /// Releases everything held at `timestamp`, firing nothing a release would, and cancels
    /// sequences in progress: a double tap waiting for its second press goes idle then.
    pub fn release_all(&mut self, timestamp: Timestamp) {
        self.push(timestamp, RawInput::ReleaseAll);
    }

    /// Processes the queued input, then brings time-driven triggers up to `now`, a time on the
    /// platform's event clock no earlier than the queued input's.
    ///
    /// Allocates nothing, unless more input was queued, or more action events happened, than
    /// ever before (both lists keep their capacity).
    pub fn update(&mut self, now: Timestamp) {
        profiling::scope!("input::update");
        self.events.clear();
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

    pub fn state(&self, action: ActionId) -> &ActionState {
        &self.actions[action.index()]
    }

    /// The kind of device the player used last: for prompt glyphs.
    pub fn last_device(&self) -> Option<DeviceKind> {
        self.devices.last()
    }

    /// Gamepads connected now; on-screen controls hide while there is one (spec §6.6).
    pub fn gamepads(&self) -> usize {
        self.devices.gamepads()
    }

    /// Makes the next key, mouse button, gamepad button or pushed axis a capture for
    /// rebinding instead of input: [`take_captured`](Self::take_captured) returns it.
    pub fn capture_next(&mut self) {
        self.capture_armed = true;
        self.captured = None;
    }

    /// The source captured since [`capture_next`](Self::capture_next), once there is one.
    pub fn take_captured(&mut self) -> Option<Source> {
        self.captured.take()
    }

    /// Makes the binding named `name` read `source`, recompiling the map, and returns the
    /// source it read before. Enabled contexts and action phases carry over; every binding
    /// starts afresh, so anything held is ignored until released.
    pub fn rebind(&mut self, name: &str, source: Source) -> Result<Source, RebindError> {
        let (document, previous) =
            self.map.rebound(name, source).ok_or_else(|| RebindError::UnknownBinding(name.to_owned()))?;
        self.replace_map(InputMap::new(document)?);
        Ok(previous)
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
        self.devices.reset_virtuals(map.virtual_count());
        self.map = map;
        self.contexts_changed = true;
    }

    /// Captures `source` if a capture is armed; returns whether it did.
    fn capture(&mut self, source: impl FnOnce() -> Source) -> bool {
        if !self.capture_armed {
            return false;
        }
        self.capture_armed = false;
        self.captured = Some(source());
        true
    }

    fn apply(&mut self, now: Timestamp, input: RawInput) {
        match input {
            RawInput::Key { scancode, down } => {
                if down && self.capture(|| Source::Key(Key::from_scancode(scancode))) {
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
                if down && self.capture(|| Source::Mouse(button)) {
                    return;
                }
                if down {
                    self.devices.used(DeviceKind::KeyboardMouse);
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
                if down && self.capture(|| Source::Pad(button)) {
                    return;
                }
                if self.devices.set_pad_button(pad, button, down) {
                    self.evaluate(now, MotionPass::Keep, Abort::No);
                }
            }
            RawInput::PadAxis { pad, axis, value } => {
                // Captured but not swallowed: the axis must keep reading true.
                if value.abs() >= CAPTURE_AXIS {
                    self.capture(|| Source::PadAxis(axis));
                }
                if self.devices.set_pad_axis(pad, axis, value) {
                    self.evaluate(now, MotionPass::Keep, Abort::No);
                }
            }
            RawInput::Touch { finger, phase, position } => {
                if phase == TouchPhase::Down {
                    self.devices.used(DeviceKind::Touch);
                }
                let out = self.gestures.touch(now, finger, phase, position, self.map.gestures());
                if let Some(change) = out.press
                    && self.devices.set_press(change == PressChange::Down)
                {
                    let abort = if change == PressChange::Cancel { Abort::Release } else { Abort::No };
                    self.evaluate(now, MotionPass::Keep, abort);
                }
                if let Some(drag) = out.drag {
                    self.add_motion(Channel::Drag, drag, now);
                }
                if let Some(pinch) = out.pinch {
                    self.add_motion(Channel::Pinch, Vec2::new(pinch, 0.0), now);
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
                if self.devices.set_virtual(control.index(), value) {
                    self.evaluate(now, MotionPass::Keep, Abort::No);
                }
            }
            RawInput::ReleaseAll => {
                self.devices.release_all();
                self.gestures.reset();
                self.evaluate(now, MotionPass::Keep, Abort::All);
            }
        }
    }

    /// Adds motion; the first of an update is judged at once, so its action starts at that
    /// motion's timestamp. The rest only add up until the update's final evaluation.
    fn add_motion(&mut self, channel: Channel, delta: Vec2, now: Timestamp) {
        if self.devices.add_motion(channel, delta, self.frame) {
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
            self.evaluate(deadline.max(self.time), MotionPass::Keep, Abort::No);
        }
    }

    /// Judges every active binding at `now` and turns the changes into action events.
    fn evaluate(&mut self, now: Timestamp, pass: MotionPass, abort: Abort) {
        self.time = now;
        let map = &self.map;
        for &action in map.order() {
            let (start, end) = self.action_ranges[action.index()];
            let mut combined = Combined::default();
            for &index in &self.active[start..end] {
                let binding = &map.bindings()[index];
                let state = &mut self.bindings[index];
                if !pass.reads(binding.source) {
                    combined.add(state.output);
                    continue;
                }
                let raw = self.devices.value(binding.source, self.frame);
                let value = modifier::apply_all(map.modifiers(binding), raw, self.dt);
                let actuated = is_actuated(value, binding.threshold);
                let fire = trigger::evaluate(binding.trigger, &mut state.memory, actuated, now, abort);
                let triggered = |chord: ActionId| self.actions[chord.index()].phase == ActionPhase::Triggered;
                let gated = binding.chord.is_some_and(|chord| !triggered(chord))
                    || self.blockers[state.blockers.0..state.blockers.1]
                        .iter()
                        .any(|&chord| triggered(chord));
                state.output = if gated {
                    Output::default()
                } else {
                    Output { state: fire.state, pulse: fire.pulse, value }
                };
                combined.add(state.output);
            }
            let kind = map.actions()[action.index()].kind;
            transition(&mut self.actions[action.index()], action, kind, combined, now, &mut self.events);
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
                    let binding = &map.bindings()[index];
                    let raw = self.devices.value(binding.source, self.frame);
                    let value = modifier::apply_all(map.modifiers(binding), raw, self.dt);
                    state.memory = Memory::entering(is_actuated(value, binding.threshold));
                    state.output = Output::default();
                }
                (true, false) => {
                    state.memory = Memory::default();
                    state.output = Output::default();
                }
                _ => {}
            }
            state.active = state.include;
        }
    }
}

fn is_actuated(value: Vec2, threshold: f32) -> bool {
    let magnitude = value.length();
    magnitude > 0.0 && magnitude >= threshold
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
        emit(Triggered, shaped(kind, combined.value + combined.pulse_value, true));
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
