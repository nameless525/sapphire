//! A menu's controls (M3 U5): keyboard, gamepad, mouse and fingers, as one [`UiInput`] a frame.
//!
//! The menu has its own [`InputSystem`], with its own map ([`MENU_BINDINGS`]), so a game's bindings and
//! a menu's never meet, and opening one is reading the other. What a direction means is `sapphire_ui`'s:
//! here it's only made one press at a time.

use sapphire_input::{
    ActionEventKind, ActionId, InputBindings, InputMap, InputSystem, MapError, Pointer, PointerPhase,
    RawInput, Timestamp,
};
use sapphire_math::Vec2;
use sapphire_platform::Event;
use sapphire_ui::{UiInput, UiPointer, UiPointerKind, UiPointerPhase, UiStep};

/// The default bindings, compiled in.
pub const MENU_BINDINGS: &str = include_str!("../menu_bindings.ron");

/// The context [`MENU_BINDINGS`] binds in.
pub const MENU_CONTEXT: &str = "menu";

/// A direction held this long, in nanoseconds, starts to repeat.
const REPEAT_AFTER: u64 = 400_000_000;

/// And comes round this often, in nanoseconds.
const REPEAT_EVERY: u64 = 120_000_000;

/// How far a stick goes before it's a direction, and how far back before it isn't. The gap between them
/// is what keeps one held near the line from pressing again and again.
const STICK_ON: f32 = 0.5;
const STICK_OFF: f32 = 0.35;

/// The default bindings as a document, to rebind by editing.
pub fn menu_bindings() -> Result<InputBindings, MapError> {
    Ok(sapphire_serialize::from_ron(MENU_BINDINGS)?)
}

/// What a menu can't start with.
#[derive(Debug, thiserror::Error)]
pub enum MenuError {
    #[error(transparent)]
    Map(#[from] MapError),
    #[error("the bindings have no action or context `{0}`")]
    Missing(&'static str),
}

/// One menu's keyboard, gamepad, mouse and fingers, read as one [`UiInput`] a frame.
#[derive(Debug)]
pub struct MenuControls {
    input: InputSystem,
    moves: ActionId,
    confirm: ActionId,
    back: ActionId,
    scroll: ActionId,
    /// The direction held, and when it next comes round.
    held: Option<(UiStep, Timestamp)>,
    pointers: Vec<UiPointer>,
    player_scale: f32,
}

impl MenuControls {
    /// Controls on the default bindings.
    pub fn with_defaults() -> Result<Self, MenuError> {
        Self::new(InputMap::new(menu_bindings()?)?)
    }

    /// Controls on `map`, which has [`MENU_BINDINGS`]' actions and context, however it binds them.
    pub fn new(map: InputMap) -> Result<Self, MenuError> {
        let action = |name| map.action(name).ok_or(MenuError::Missing(name));
        let (moves, confirm, back, scroll) =
            (action("ui_move")?, action("ui_confirm")?, action("ui_back")?, action("ui_scroll")?);
        let context = map.context(MENU_CONTEXT).ok_or(MenuError::Missing(MENU_CONTEXT))?;
        let mut input = InputSystem::new(map);
        input.enable_context(context, 0);
        Ok(Self {
            input,
            moves,
            confirm,
            back,
            scroll,
            held: None,
            pointers: Vec::with_capacity(32),
            player_scale: 1.0,
        })
    }

    /// The input system: the last-used device, and a remap's capture.
    pub fn input(&self) -> &InputSystem {
        &self.input
    }

    /// The input system, for rebinding and a remap's capture.
    pub fn input_mut(&mut self) -> &mut InputSystem {
        &mut self.input
    }

    /// The player's own UI scale (spec §6.8), which a pointer's position is divided by to reach UI
    /// points: what `UiMetrics::of_window` was given.
    pub fn set_player_scale(&mut self, scale: f32) {
        self.player_scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    }

    /// Queues a platform event for the next [`Self::update`].
    pub fn handle_event(&mut self, event: &Event) {
        self.input.handle_event(event);
    }

    /// Queues raw input for the next [`Self::update`], as a test or a replay does.
    pub fn push(&mut self, at: Timestamp, input: RawInput) {
        self.input.push(at, input);
    }

    /// What the player asked of the screen since the last update, for `UiTree::input`.
    pub fn update(&mut self, now: Timestamp) -> UiInput<'_> {
        profiling::scope!("MenuControls::update");
        self.input.update(now);
        let step = self.step(now);
        self.pointers.clear();
        for pointer in self.input.pointers() {
            let (id, kind) = match pointer.pointer {
                Pointer::Mouse => (u32::MAX, UiPointerKind::Mouse),
                Pointer::Finger(finger) => (finger as u32, UiPointerKind::Touch),
            };
            let phase = match pointer.phase {
                PointerPhase::Down => UiPointerPhase::Down,
                PointerPhase::Moved => UiPointerPhase::Move,
                PointerPhase::Up => UiPointerPhase::Up,
                PointerPhase::Canceled => UiPointerPhase::Cancel,
            };
            self.pointers.push(UiPointer {
                id,
                kind,
                phase,
                position: pointer.position / self.player_scale,
                at: pointer.timestamp,
            });
        }
        UiInput {
            step,
            confirm: self.input.state(self.confirm).fire_count() > 0,
            back: self.input.state(self.back).fire_count() > 0,
            scroll: self.input.state(self.scroll).value(),
            pointers: &self.pointers,
        }
    }

    /// The direction this frame presses, if any: at once, then after a wait, then steadily. At most one
    /// a frame, however late the frame.
    fn step(&mut self, now: Timestamp) -> Option<UiStep> {
        let value = self.input.state(self.moves).value();
        let Some(direction) = direction_of(value, self.held.map(|(step, _)| step)) else {
            self.held = None;
            // Nothing's held now, and a tap may have come and gone inside the frame.
            return self
                .input
                .events()
                .iter()
                .filter(|event| event.action == self.moves && event.kind != ActionEventKind::Completed)
                .find_map(|event| direction_of(event.value, None));
        };
        let after = |nanos: u64| Timestamp::from_nanos(now.as_nanos() + nanos);
        match self.held {
            Some((step, next)) if step == direction => {
                if now < next {
                    return None;
                }
                self.held = Some((step, after(REPEAT_EVERY)));
            }
            _ => self.held = Some((direction, after(REPEAT_AFTER))),
        }
        Some(direction)
    }
}

/// How far `step` is along `value`.
fn along(step: UiStep, value: Vec2) -> f32 {
    match step {
        UiStep::Up => value.y,
        UiStep::Down => -value.y,
        UiStep::Left => -value.x,
        UiStep::Right => value.x,
    }
}

/// The direction `value` means: `held`, while it's still that way, or its larger axis.
fn direction_of(value: Vec2, held: Option<UiStep>) -> Option<UiStep> {
    if let Some(held) = held
        && along(held, value) >= STICK_OFF
    {
        return Some(held);
    }
    let step = if value.x.abs() > value.y.abs() {
        if value.x > 0.0 { UiStep::Right } else { UiStep::Left }
    } else if value.y > 0.0 {
        UiStep::Up
    } else {
        UiStep::Down
    };
    (along(step, value) >= STICK_ON).then_some(step)
}
