//! Action mapping on the model of Unreal's Enhanced Input (spec §5.18, layer L3).
//!
//! # Owns
//!
//! - **One device-agnostic pipeline.** Keyboard, mouse, gamepads, touch and on-screen controls
//!   all become [`RawInput`], read by bindings through one vocabulary of [`Source`]s.
//!   [`InputSystem::handle_event`] translates `sapphire_platform` events.
//! - **Contexts and actions.** An [`InputMap`] holds actions (button, axis1D, axis2D) and
//!   contexts that bind sources to them. Enabled contexts form a priority stack: a context
//!   whose actions consume their sources hides those sources from every lower context.
//! - **Modifiers** ([`Modifier`]): axial and radial dead zones, response curves, negation,
//!   scaling, swizzles, and scaling by the update's length.
//! - **Triggers** ([`Trigger`]): down, pressed, released, hold, tap and double tap, plus chords
//!   (a binding's `chord`: another action that must be triggered). A chord also blocks the
//!   unchorded bindings of its source while it is held, so `Ctrl+S` doesn't fire `S`.
//! - **Touch gestures** ([`Gesture`]): press (so taps, double taps and long presses come from
//!   the triggers), one-finger drag, two-finger pinch, and swipes by direction. On-screen
//!   controls inject their values ([`InputSystem::inject`]) into [`Source::Virtual`]s, which
//!   bind like any other device.
//! - **Bindings as data:** [`InputBindings`] is a RON document (`sapphire_serialize`,
//!   `sapphire::input::InputBindings`, schema version 1), registered for loading by name.
//!   [`InputSystem::rebind`] rebinds a named binding at runtime, and
//!   [`InputSystem::capture_next`] captures the source a player presses for it.
//! - **The last-used device** ([`DeviceKind`], with the gamepad's [`GamepadFamily`]), for
//!   prompt glyphs.
//!
//! # Does not own
//!
//! - Device access and event timestamps: `sapphire_platform`.
//! - Intents: gameplay turns action events into intents in the fixed step (M2's intent buffer).
//! - On-screen controls' layout and drawing, and routing touches to them: `game_ui` on
//!   `sapphire_ui` (M3).
//! - Persisting rebinds: user settings (`sapphire_save`, M3). [`InputSystem::bindings`] is the
//!   document they will store.
//!
//! # Time
//!
//! The crate never reads a clock (rule R2). Input time is the platform's event clock
//! ([`Timestamp`], SDL's nanoseconds): every raw input carries its device event's timestamp,
//! and the app passes [`Platform::now`](sapphire_platform::Platform::now) to
//! [`InputSystem::update`] once per frame. The update replays the frame's input in order and
//! judges every binding at each input's own timestamp, and at every trigger deadline in between,
//! so:
//!
//! - every [`ActionEvent`] carries the timestamp of the device event that caused it, or, for a
//!   change time alone caused, the exact instant (a hold completes at its press plus the hold
//!   time), whenever the frame happened to run;
//! - a press and release within one frame both count, which matters at 30 fps;
//! - tests drive everything with synthetic timestamps.
//!
//! Gameplay reads the events after the update and hands them to the next fixed step, where
//! queued input lands on the first step (rule R3); the timestamps let it judge input buffers
//! against real time. Motion sources (mouse motion, wheel, drag, pinch) are summed per update
//! instead of judged per event, since their value is "how far this frame". Each motion binding
//! sums only the motion that arrives while its chord is held and no chord blocks it, and keeps
//! that sum for the rest of the update even if the chord lets go: a drag counts the motion
//! from its press to its release, wherever the frame boundaries fall, and its action starts at
//! the first motion it takes.
//!
//! Motion is an impulse. A motion binding is actuated in the updates that gave it motion, and
//! stays actuated from one such update to the next, so an edge trigger on motion fires once
//! per run of consecutive updates with motion, not once per event: `Pressed` as the run
//! begins, `Released` and `Tap` with the first update after it. Evaluations for other input
//! never fire it again.
//!
//! # Allocation
//!
//! Steady-state frames allocate nothing: device state is fixed-size, the queue and event lists
//! keep their capacity, and the active binding set is rebuilt into preallocated lists when
//! contexts change. Exceptions, all outside the frame's steady state: the first frames that
//! queue more input or produce more events than ever before grow those lists once, and
//! rebinding recompiles the map.
//!
//! Measured on the sandbox's 19 bindings (M1 F4, Apple M5, release): an update costs about
//! 0.2 µs without input and 1 µs with five inputs.
//!
//! # Later milestones
//!
//! - Rumble and trigger rumble as cues (M2, with `sapphire_audio`'s cue system); DualSense LEDs
//!   and gyro, and gamepad touchpads.
//! - An ECS resource and the intent buffer hand-off, with `sapphire_gameplay` (M2).
//! - Persisted rebinds and accessibility options, such as hold-to-toggle (spec §6.8), with user
//!   settings (M3); glyph tables with the UI (M3).
//! - Hot reload of bindings documents with the asset pipeline (A8): [`InputBindings`] already
//!   loads through `sapphire_serialize::from_ron_dynamic` once registered ([`register_types`]).

mod bindings;
mod devices;
mod gesture;
mod map;
mod modifier;
mod raw;
mod source;
mod system;
mod trigger;

pub use bindings::{ActionDesc, ActionKind, BindingDesc, ContextDesc, GestureSettings, InputBindings};
pub use devices::{DeviceKind, GamepadFamily, MAX_FINGERS, MAX_GAMEPADS};
pub use map::{ActionId, ContextId, InputMap, MapError, VirtualControl};
pub use modifier::{Axes, DeadZoneShape, Modifier};
pub use raw::RawInput;
pub use sapphire_platform::Timestamp;
pub use source::{Gesture, Key, MouseButton, PadAxis, PadButton, Source, Stick, SwipeDirection};
pub use system::{
    ActionEvent, ActionEventKind, ActionPhase, ActionState, InputSystem, MAX_UPDATE_DELTA, RebindError,
};
pub use trigger::Trigger;

/// Registers the crate's document types, so `sapphire_serialize::from_ron_dynamic` can load
/// bindings by type name (hot reload, live link, the editor).
pub fn register_types(registry: &mut sapphire_reflect::TypeRegistry) {
    registry.register::<InputBindings>();
}
