//! Touch zones: which finger drives which control, from press to release, as plain action data.
//!
//! # Ownership
//!
//! A finger belongs to what it landed on until it lifts:
//!
//! - **Buttons first.** A landing inside a button's hit circle takes the nearest such button. A
//!   thumb that slides off keeps it: `over` says where the thumb is, the press holds, and sliding
//!   back changes nothing. A second finger on a held button takes nothing.
//! - **Then the stick,** a landing in its zone while no finger has it. The floating stick appears
//!   under the thumb. A second finger there falls through to the camera.
//! - **Then the camera,** anywhere else, the frame's whole area, while no finger has it. Further
//!   fingers take nothing, but they are still the interface's, so `sapphire_input`'s gestures
//!   never see half of a sequence.
//! - A finger never changes owner: the camera's never becomes the stick, however it moves, and
//!   a control that moves under a finger (a new layout) keeps it.
//!
//! # Letting go, as `sapphire_input` does
//!
//! - **A finger lifting** releases what it holds: a [`ButtonEventKind::Released`] with the press's
//!   drag.
//! - **The OS taking a finger back** (`TouchPhase::Canceled`, a system gesture) ends only what
//!   that finger holds, with [`ButtonEventKind::Canceled`] and nothing a release would fire, as
//!   `Abort::Release` leaves other devices alone: the other thumb's stick carries on.
//! - **Losing focus or the foreground** ([`TouchControls::release_all`]) ends everything the same
//!   way, as `Abort::All` does, and forgets every finger at once, since their releases may never
//!   come. A late event from one finds nobody, and isn't the interface's.
//! - **Motion keeps what it took.** The camera's delta for an update keeps what arrived before a
//!   cancel or a loss in that update, as a drag's action does in `sapphire_input` (FI-002, WA-4).
//!
//! # Time
//!
//! The crate reads no clock (rule R2). Events carry the platform's timestamps, and the app passes
//! [`Platform::now`](sapphire_platform::Platform::now) to [`TouchControls::update`], on the same
//! clock; tests and replays pass their own. A press and its release in one frame both count, which
//! matters at 30 fps, and [`ButtonEvent`]s keep their order and their own timestamps. A hold's
//! [`ButtonEventKind::Held`] carries the exact instant it completed, the press plus the hold time,
//! whenever the frame happened to run.

use std::time::Duration;

use sapphire_math::Vec2;
use sapphire_platform::{Event, FingerId, LifecycleEvent, Timestamp, TouchPhase, WindowEvent};

use crate::hud::HudLayout;

/// Fingers followed at once; further landings take nothing. `sapphire_input`'s `MAX_FINGERS`.
pub const MAX_TOUCHES: usize = 10;

/// How far a held button's thumb travels, in points, before it counts as a drag.
const DRAG_SLOP: f32 = 10.0;

/// An on-screen button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Attack,
    Dodge,
    Skill,
}

impl Button {
    pub const ALL: [Self; 3] = [Self::Attack, Self::Dodge, Self::Skill];

    const fn index(self) -> usize {
        self as usize
    }
}

/// What happened to a button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ButtonEventKind {
    /// A finger landed on it.
    Pressed,
    /// It has been held for its hold time ([`TouchTuning::attack_hold`]).
    Held,
    /// Its finger lifted.
    Released,
    /// It let go with nothing a release would fire: the OS took the finger back, or focus or the
    /// foreground was lost.
    Canceled,
}

/// One edge on a button, in the order they happened.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ButtonEvent {
    pub button: Button,
    pub kind: ButtonEventKind,
    /// When, on the platform's event clock.
    pub at: Timestamp,
    /// How far its thumb had moved from where it landed, in points, +y up.
    pub drag: Vec2,
}

/// A button's state as of the last update.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ButtonState {
    /// A finger holds it.
    pub down: bool,
    /// When that finger landed; meaningful while `down`.
    pub pressed_at: Timestamp,
    /// How far the thumb has moved from where it landed, in points, +y up. Zero when not `down`.
    pub drag: Vec2,
    /// The thumb has gone past the drag slop since it landed, and stays so until it lifts.
    pub dragging: bool,
    /// The thumb is inside the hit circle. A thumb outside it still holds the button.
    pub over: bool,
    /// Its hold time has passed since it was pressed.
    pub held: bool,
    /// It was pressed since the last update, whether or not it's still down.
    pub pressed: bool,
    /// It was released since the last update.
    pub released: bool,
    /// It was canceled since the last update.
    pub canceled: bool,
}

impl ButtonState {
    /// How long it has been held at `now`, if it's down.
    pub fn held_for(&self, now: Timestamp) -> Option<Duration> {
        self.down.then(|| now.saturating_duration_since(self.pressed_at))
    }
}

/// The floating stick's state as of the last update.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StickState {
    /// A thumb is on it.
    pub active: bool,
    /// Where it points and how far, within the unit circle, +y up, past its dead zone.
    pub value: Vec2,
    /// Its middle, in points: under the thumb at first, following it if it runs past the radius.
    pub center: Vec2,
    /// The thumb, in points.
    pub thumb: Vec2,
}

/// What the app reads from the touch controls after each update: plain values, for its bindings or
/// its intents.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TouchActions {
    pub stick: StickState,
    /// How far the camera's finger moved since the last update, in points, +y up.
    pub camera_delta: Vec2,
    pub buttons: [ButtonState; Button::ALL.len()],
    /// Every button's edges since the last update, in the order they happened.
    pub events: Vec<ButtonEvent>,
}

impl TouchActions {
    pub fn button(&self, button: Button) -> &ButtonState {
        &self.buttons[button.index()]
    }
}

/// What a finger holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner {
    Stick,
    Button(Button),
    Camera,
    /// Nothing: it landed where nothing was free.
    Nothing,
}

#[derive(Clone, Copy, Debug)]
struct Touch {
    finger: FingerId,
    owner: Owner,
    landed: Vec2,
    at: Vec2,
}

/// The tunable parts of the controls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchTuning {
    /// The share of the stick's radius that reads as zero; beyond it the value rises from zero.
    pub stick_dead_zone: f32,
    /// How long the attack button is held before [`ButtonEventKind::Held`].
    pub attack_hold: Duration,
}

impl Default for TouchTuning {
    fn default() -> Self {
        Self { stick_dead_zone: 0.1, attack_hold: Duration::from_millis(300) }
    }
}

/// The on-screen controls' input half: fingers, zones and what the app reads. It has no GPU or
/// platform dependency to run: tests and the replay suite drive it with [`Self::touch`].
#[derive(Clone, Debug)]
pub struct TouchControls {
    layout: HudLayout,
    tuning: TouchTuning,
    touches: [Option<Touch>; MAX_TOUCHES],
    /// The state as input arrives, published by [`Self::update`].
    pending: TouchActions,
    actions: TouchActions,
}

impl TouchControls {
    pub fn new(layout: HudLayout) -> Self {
        Self::with_tuning(layout, TouchTuning::default())
    }

    pub fn with_tuning(layout: HudLayout, tuning: TouchTuning) -> Self {
        Self {
            layout,
            tuning,
            touches: [None; MAX_TOUCHES],
            pending: TouchActions::default(),
            actions: TouchActions::default(),
        }
    }

    /// Takes a new layout: from the next landing on. Fingers down keep what they hold.
    pub fn set_layout(&mut self, layout: HudLayout) {
        self.layout = layout;
    }

    pub fn layout(&self) -> &HudLayout {
        &self.layout
    }

    /// Takes a platform event. Returns whether it was a touch the controls took, which nothing
    /// else should see then; everything but a late touch from a forgotten finger is one. A window
    /// losing focus and the app leaving the foreground let go of everything and return `false`,
    /// since they're for everyone.
    pub fn handle_event(&mut self, event: &Event) -> bool {
        match *event {
            Event::Touch(touch) => {
                let position = Vec2::new(touch.x, touch.y) * self.layout.metrics.size;
                self.touch(touch.timestamp, touch.finger, touch.phase, position)
            }
            Event::Window { timestamp, event: WindowEvent::FocusLost, .. }
            | Event::Lifecycle { timestamp, event: LifecycleEvent::WillEnterBackground } => {
                self.release_all(timestamp);
                false
            }
            _ => false,
        }
    }

    /// One finger's event at `position` in UI points. Returns whether the controls took it: all
    /// landings do, and every later event of a finger they know.
    pub fn touch(&mut self, at: Timestamp, finger: FingerId, phase: TouchPhase, position: Vec2) -> bool {
        self.fire_holds(at);
        let slot = self.touches.iter().position(|touch| touch.is_some_and(|t| t.finger == finger));
        match phase {
            TouchPhase::Down => {
                // Its last release never came. What it held ends as if canceled.
                if let Some(slot) = slot {
                    self.let_go(slot, at, ButtonEventKind::Canceled);
                }
                self.land(at, finger, position);
                true
            }
            TouchPhase::Moved => {
                let Some(slot) = slot else { return false };
                self.moved(slot, position);
                true
            }
            TouchPhase::Up => {
                let Some(slot) = slot else { return false };
                self.moved(slot, position);
                self.let_go(slot, at, ButtonEventKind::Released);
                true
            }
            // Where a finger was when the OS took it back says nothing about what it meant.
            TouchPhase::Canceled => {
                let Some(slot) = slot else { return false };
                self.let_go(slot, at, ButtonEventKind::Canceled);
                true
            }
        }
    }

    /// Everything lets go at `at`, firing nothing a release would, and every finger is forgotten:
    /// focus or the foreground was lost, so the real releases may never come.
    pub fn release_all(&mut self, at: Timestamp) {
        self.fire_holds(at);
        for slot in 0..MAX_TOUCHES {
            if self.touches[slot].is_some() {
                self.let_go(slot, at, ButtonEventKind::Canceled);
            }
        }
    }

    /// Publishes what arrived since the last update, as of `now` on the event clock: holds that
    /// have come due fire, and the edges and the camera's delta start over. Returns what the app
    /// reads, also at [`Self::actions`].
    pub fn update(&mut self, now: Timestamp) -> &TouchActions {
        self.fire_holds(now);
        let TouchActions { stick, camera_delta, buttons, events } = &mut self.actions;
        *stick = self.pending.stick;
        *camera_delta = self.pending.camera_delta;
        *buttons = self.pending.buttons;
        // Swapped, not cloned: each list keeps its capacity.
        std::mem::swap(events, &mut self.pending.events);
        self.pending.events.clear();
        self.pending.camera_delta = Vec2::ZERO;
        for button in &mut self.pending.buttons {
            button.pressed = false;
            button.released = false;
            button.canceled = false;
        }
        &self.actions
    }

    /// The state as of the last update.
    pub fn actions(&self) -> &TouchActions {
        &self.actions
    }

    /// The fingers followed right now.
    pub fn fingers(&self) -> usize {
        self.touches.iter().flatten().count()
    }

    /// Whether this finger holds the stick or a button, for whatever else counts fingers: the camera's
    /// finger and one that took nothing are free to mean something else.
    pub fn holds_control(&self, finger: FingerId) -> bool {
        self.touches
            .iter()
            .flatten()
            .any(|touch| touch.finger == finger && matches!(touch.owner, Owner::Stick | Owner::Button(_)))
    }

    fn owned(&self, owner: Owner) -> bool {
        self.touches.iter().flatten().any(|touch| touch.owner == owner)
    }

    /// The first free owner at `position`, by the order the module docs give.
    fn claim(&self, position: Vec2) -> Owner {
        let layout = &self.layout;
        let on_button = Button::ALL
            .into_iter()
            .filter_map(|button| {
                let zone = layout.button(button);
                let distance = zone.center.distance(position);
                (distance <= zone.hit).then_some((button, distance))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((button, _)) = on_button {
            return if self.owned(Owner::Button(button)) { Owner::Nothing } else { Owner::Button(button) };
        }
        if layout.stick.zone.contains(position) && !self.owned(Owner::Stick) {
            return Owner::Stick;
        }
        if self.owned(Owner::Camera) { Owner::Nothing } else { Owner::Camera }
    }

    fn land(&mut self, at: Timestamp, finger: FingerId, position: Vec2) {
        // Past the slots a finger takes nothing, and its later events find nobody.
        let Some(slot) = self.touches.iter().position(Option::is_none) else { return };
        let owner = self.claim(position);
        self.touches[slot] = Some(Touch { finger, owner, landed: position, at: position });
        match owner {
            Owner::Stick => {
                self.pending.stick =
                    StickState { active: true, value: Vec2::ZERO, center: position, thumb: position };
            }
            Owner::Button(button) => {
                self.pending.buttons[button.index()] = ButtonState {
                    down: true,
                    pressed_at: at,
                    over: true,
                    pressed: true,
                    // An earlier release in this update stays on the record.
                    released: self.pending.buttons[button.index()].released,
                    canceled: self.pending.buttons[button.index()].canceled,
                    ..ButtonState::default()
                };
                self.event(button, ButtonEventKind::Pressed, at, Vec2::ZERO);
            }
            Owner::Camera | Owner::Nothing => {}
        }
    }

    fn moved(&mut self, slot: usize, position: Vec2) {
        let Some(touch) = &mut self.touches[slot] else { return };
        let (owner, last, landed) = (touch.owner, touch.at, touch.landed);
        touch.at = position;
        match owner {
            Owner::Stick => {
                let radius = self.layout.stick.radius;
                let dead_zone = self.tuning.stick_dead_zone;
                let stick = &mut self.pending.stick;
                stick.thumb = position;
                let mut from_center = position - stick.center;
                // Running past the rim drags the stick along, so a thumb that drifts doesn't lose
                // its direction when it turns back.
                if from_center.length() > radius {
                    from_center = from_center.normalize() * radius;
                    stick.center = position - from_center;
                }
                stick.value = stick_value(Vec2::new(from_center.x, -from_center.y) / radius, dead_zone);
            }
            Owner::Button(button) => {
                let zone = self.layout.button(button);
                let (center, hit) = (zone.center, zone.hit);
                let state = &mut self.pending.buttons[button.index()];
                let moved = position - landed;
                state.drag = Vec2::new(moved.x, -moved.y);
                state.dragging |= moved.length() > DRAG_SLOP;
                state.over = center.distance(position) <= hit;
            }
            Owner::Camera => {
                let delta = position - last;
                self.pending.camera_delta += Vec2::new(delta.x, -delta.y);
            }
            Owner::Nothing => {}
        }
    }

    /// Ends what the finger in `slot` holds, and forgets it.
    fn let_go(&mut self, slot: usize, at: Timestamp, kind: ButtonEventKind) {
        let Some(touch) = self.touches[slot].take() else { return };
        match touch.owner {
            Owner::Stick => self.pending.stick = StickState::default(),
            Owner::Button(button) => {
                let state = &mut self.pending.buttons[button.index()];
                let drag = state.drag;
                *state = ButtonState {
                    pressed: state.pressed,
                    released: state.released || kind == ButtonEventKind::Released,
                    canceled: state.canceled || kind == ButtonEventKind::Canceled,
                    ..ButtonState::default()
                };
                self.event(button, kind, at, drag);
            }
            Owner::Camera | Owner::Nothing => {}
        }
    }

    /// Fires every hold that has come due by `at`, at its own instant.
    fn fire_holds(&mut self, at: Timestamp) {
        let hold = self.tuning.attack_hold;
        let state = &mut self.pending.buttons[Button::Attack.index()];
        if !state.down || state.held || at.saturating_duration_since(state.pressed_at) < hold {
            return;
        }
        state.held = true;
        let due = Timestamp::from_nanos(state.pressed_at.as_nanos().saturating_add(hold.as_nanos() as u64));
        let drag = state.drag;
        self.event(Button::Attack, ButtonEventKind::Held, due, drag);
    }

    fn event(&mut self, button: Button, kind: ButtonEventKind, at: Timestamp, drag: Vec2) {
        self.pending.events.push(ButtonEvent { button, kind, at, drag });
    }
}

/// A position in the stick's unit circle past `dead_zone`, rising from zero at its edge to one at
/// the rim.
fn stick_value(position: Vec2, dead_zone: f32) -> Vec2 {
    let length = position.length();
    if length <= dead_zone || dead_zone >= 1.0 {
        return Vec2::ZERO;
    }
    position / length * ((length - dead_zone) / (1.0 - dead_zone)).min(1.0)
}

#[cfg(test)]
mod tests;
