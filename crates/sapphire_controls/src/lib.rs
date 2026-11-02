//! Combat's controls (M2 I1, spec §5.18): a gamepad or keyboard driving the simulation.
//!
//! Sits above `sapphire_input` and `sapphire_gameplay`, which may not know each other: the
//! simulation has no input in its tree, so that a replay builds without one. Any game on the engine
//! needs what's here, so it's a crate rather than part of an app.
//!
//! # Owns
//!
//! - **The default bindings** ([`DEFAULT_BINDINGS`], `combat_bindings.ron`), with gameplay and
//!   pause contexts. Rebinding is editing that document, or [`InputSystem::rebind`] at run time by
//!   a binding's name: `light.gamepad`, `dodge.keyboard`.
//! - **The mapping from input to [`TickActions`]** ([`CombatControls`]), called once per frame and
//!   once per fixed step, with the camera's yaw. [`ControlsSource`] is the same mapping as a replay
//!   recording's source.
//! - **The pause:** it stops the steps, not the app, and resuming takes no burst.
//!
//! # Doesn't own
//!
//! - Devices, contexts, triggers and rebinding: `sapphire_input`.
//! - The camera. [`CombatControls::look`], [`CombatControls::camera_zoom`] and
//!   [`CombatControls::raw_stick`] are presentation, for the app to apply, and aren't in a tick. The
//!   mapping takes the camera's yaw as a plain number.
//! - Touch zones (`sapphire_ui`, which hands its values to [`CombatControls::set_on_screen`]), a
//!   pause menu's visuals, and rumble.
//!
//! # No press is lost or doubled
//!
//! Input is judged per frame and read per fixed step, and a frame runs 0, 1 or 2 steps. Every
//! press of a verb, however brief, is one rising edge in the sequence of steps, which is how the
//! simulation sees presses (`sapphire_gameplay::Controls::pressed`):
//!
//! - A tap that starts and ends inside a frame, or between steps, reads as held on exactly one step.
//! - At two steps a frame, a press held through both is one edge, and a tap reads down, then up.
//! - A frame with no step keeps what it saw for the next one.
//! - Two presses too quick for the steps each get their edge, one step apart from the last: up,
//!   down, up. At most four wait per button ([`ControlsStats::presses_over_limit`]).
//!
//! What ends a press, by `sapphire_input`'s rules (its `Abort`):
//!
//! | What happened | Held buttons and the stick | A press already made | `Tap`, `DoubleTap` if rebound to them |
//! |---|---|---|---|
//! | The player let go | The next step reads them released | Arrives | A tap fires on its release |
//! | A pad disconnected, or the OS took a touch back (`Abort::Release`) | That device's let go, and other devices' stay | Arrives | Nothing fires, and other devices' double taps stay armed |
//! | Focus or the foreground was lost (`Abort::All`) | All let go | Arrives | Nothing fires, and every double tap goes idle |
//!
//! # The stick's path
//!
//! For the left stick, in this order: SDL's axes as −1..=1, with +y flipped to forward; the round
//! dead zone, whose length rescales from 0 at 0.15 to 1 at 0.95; no curve; then the sum with the
//! other bindings of `move` (keys, on-screen); that sum's length limited to 1, so two keys'
//! diagonal is no faster than one; the turn by the camera's yaw, from the view into the world; and
//! [`Stick::from_axes`](sapphire_gameplay::Stick::from_axes). So a stick at the end of its range has
//! a length of 127 steps, even one too worn to quite reach it, and a centred one, or one resting
//! anywhere in its dead zone, reads exactly 0. The stick is a level: every step reads the latest
//! update's value. So nothing is lost when a level comes and goes between steps, which only keys and
//! on-screen controls are quick enough to do, the next step reads what it was, if the stick reads
//! nothing else by then.
//!
//! **A tick's stick is world-space:** +x is world +X and +y is world +Z, so the simulation never
//! reads a camera and a replay carries the direction the player meant. A fully tilted diagonal is a
//! length of 1 by the time it turns, because the limit comes first, so no axis clips: W and D under
//! a camera along +Z read (−90, 90), and under a camera turned 45° they read (0, 127), the same
//! speed. Each axis rounds on its own, so a full tilt's length is within half a step per axis of 127
//! steps: at most 1.0056 through `Stick::to_vec2`. Whatever reads a speed from it limits that itself.
//!
//! # The pause
//!
//! [`CombatControls::advance`] returns no steps while paused, and leaves the driver alone, so the
//! accumulator doesn't fill and the steps pick up as if the pause hadn't happened. The app's frame
//! goes on: its overlay, hot reload and live link don't run on steps. A time scale can't do this:
//! the debug overlay's pause sets the world clock's scale to 0, and the steps still tick, which
//! changes the simulation's checksum. Here the simulation sees no time at all.
//!
//! The combat context gives way to the pause context, so a press during a pause never reaches the
//! game, and the context's return follows `sapphire_input`'s rule for what's held on entry: verbs
//! bound with `DownFromPress` read released until pressed again, so the button that resumed isn't
//! also an attack, and one held through the pause isn't pressed twice. A stick reads the level
//! straight away. A press made before the pause in the same frame arrives on the first step after
//! it, as if no time had passed.
//!
//! Android's Back opens the pause, and Back while paused leaves through
//! [`CombatControls::wants_to_leave`] (spec §5.18 has why).
//!
//! # Allocation
//!
//! Steady-state frames allocate nothing, pauses included (`tests/allocations.rs`).
//!
//! [`TickActions`]: sapphire_gameplay::TickActions
//! [`InputSystem::rebind`]: sapphire_input::InputSystem::rebind

mod controls;
mod latch;

pub use controls::{
    BUTTON_ACTIONS, COMBAT_CONTEXT, COMBAT_PRIORITY, CombatControls, ControlsError, ControlsSource,
    ControlsStats, DEFAULT_BINDINGS, MOVE_ACTION, PAUSE_CONTEXT, PAUSE_PRIORITY, default_bindings,
};
