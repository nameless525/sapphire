//! The headless simulation (spec §5.22, `docs/m2-plan.md` §3): a combat world that ticks with no
//! window and no GPU, and gives one checksum for one set of inputs.
//!
//! Owns:
//!
//! - [`Simulation`], which owns a world and runs one fixed step at a time: [`Simulation::step`] and
//!   [`Simulation::checksum`].
//! - [`TickActions`], a tick's input as plain data with a stable byte layout. The app maps
//!   `sapphire_input`'s actions onto it, and a replay stores it.
//! - [`ActorId`]s, never reused within a run, and pools of actors spawned ahead of time, so a spawn
//!   in the fixed step allocates nothing.
//! - [`StateHasher`], which hashes state as canonical bytes, and [`SimRng`], the seeded randomness.
//! - [`SimStats`], its counters against the budget (rule R9).
//! - [`replay`]: recorded runs as files, the scenarios they start from and the checksums they must
//!   reach, a tick at a time, for a headless runner and an app alike (spec §9, gates 2 and 4).
//! - [`targeting`]: who a soft lock picks, as a pure function, since it decides where a move warps. The
//!   camera that frames the choice is `sapphire_camera`, presentation, with no edge to this crate.
//! - The move system (M2 G2): [`IntentRing`], the intents waiting on an actor's own clock; the `moves`
//!   module's cancels and moves, on [`sapphire_moves::MoveSet`]s from [`MoveLibrary`]; what a move
//!   publishes ([`MoveOutput`], [`MoveRead`]); and [`CueQueue`], presentation's events.
//! - The character controller (the `controller` module): [`step_controller`] and
//!   [`CharacterController`], a kinematic capsule on `sapphire_physics`, on each actor's own clock. A
//!   move's root motion reaches it in [`CombatSet::Movement`] and collides as the stick's does.
//! - [`HitQuery`], which sweeps moves' hit volumes against hurtboxes, over plain inputs that
//!   [`HitQuery::sweep_published`] and [`published_hurtbox`] make from what moves publish.
//! - [`arena`]: the look-dev town's square as a place to fight: its collision, from a committed
//!   document, and where a duel starts.
//! - Resolve (M2 G3): [`Fighter`], its health, poise and push, what a landed hit comes to
//!   ([`HitOutcome`]), the reactions and hitstop it asks for, and [`SlowMotion`], world-clock slow
//!   motion. The `resolve` module's docs say how a hit lands.
//! - Perfect dodges and counters (M2 G4), through [`HitQuery::dodged`], the `resolve` module and the
//!   `moves` module's counter window.
//! - [`brain`] (M2 G6): what presses an enemy's intents and tilts its stick, on the same paths as a
//!   player's, from a section of its move set's document.
//! - The party (M2 G7): [`Party`], three slots with one on the field and the others off it and still
//!   ticking, swapping as an ability and a swap-in assist as a move, in the `party` module.
//!
//! The rest arrives with M2's other G packages, each in its [`CombatSet`].
//!
//! Doesn't own, and may never link: the renderer, the world (`sapphire_world`), presentation animation, the
//! platform layer, input bindings, SDL or wgpu. `cargo xtask check-layers` holds its dependency
//! tree to that (`[simulation]` in `xtask/layers.toml`), so a replay job builds quickly and runs
//! with no GPU. The app's scene carries results into `FramePacket` (rule R4).
//!
//! # The rules that keep it deterministic
//!
//! Replays must reproduce their checksum on every run for an architecture (spec §9, gate 2). Code
//! in this crate keeps to these:
//!
//! - **Anything that affects a result goes in [`ActorId`] order.** Walk [`ActorTable::iter`] and
//!   look each actor up, never a bare query, whose order is where its entity happens to lie:
//!
//!   ```
//!   # use sapphire_ecs::prelude::*;
//!   # use sapphire_gameplay::{ActorTable, ActorTransform};
//!   fn system(table: Res<'_, ActorTable>, mut actors: Query<'_, '_, &mut ActorTransform>) {
//!       for (_id, entity) in table.iter() {
//!           let Ok(mut transform) = actors.get_mut(entity) else { continue };
//!           transform.yaw += 0.1;
//!       }
//!   }
//!   ```
//!
//! - **Time is an owner's clock, in whole sub-ticks.** Every actor has an `EntityClock`, so hitstop
//!   and time scales reach whatever reads it.
//! - **Floats go through `sapphire_math::det`,** whose functions are pinned. This crate's
//!   `clippy.toml` bans `std`'s and glam's helpers that reach them, and `HashMap`s.
//! - **Randomness comes from [`SimRng`],** seeded from the config.
//! - **No heap allocation inside [`Simulation::step`]** (rule R6): pools for actors, fixed queues
//!   for side effects, tested from the first tick, and the replay suite fails if a step ever does.
//! - **State that can change a result goes in [`Simulation::checksum`],** which reads canonical
//!   bytes with [`StateHasher`]: a new component adds itself to the `checksum` module's loop.
//!
//! # A step
//!
//! The engine's schedules, in order: [`FixedFirst`](sapphire_ecs::FixedFirst) copies each actor's
//! actions ([`SimSet::Input`](sapphire_ecs::SimSet::Input)) and ticks the clocks;
//! [`FixedCombat`](sapphire_ecs::FixedCombat) runs [`CombatSet`], spec §5.22's order;
//! [`FixedPhysics`](sapphire_ecs::FixedPhysics) stays empty, because a capsule's move applies at once;
//! [`FixedPost`](sapphire_ecs::FixedPost) resolves the spawns and despawns queued during the step
//! (rule R3).
//!
//! # Example
//!
//! ```
//! use sapphire_gameplay::{ActorActions, ActorSpawn, Buttons, SimConfig, Simulation, Stick, TickActions};
//! use sapphire_moves::MoveSetDocument;
//!
//! let mut sim = Simulation::new(SimConfig { seed: 7, ..SimConfig::default() });
//!
//! // One move, as a cook would make it: a step forward on a button.
//! let document = MoveSetDocument::parse(r#"(
//!     type: "sapphire::moves::MoveSetDocument",
//!     version: 1,
//!     data: (
//!         intents: [(name: "light", button: Light, buffer: 12)],
//!         neutral: [(intent: "light", move: "step")],
//!         moves: [(
//!             name: "step",
//!             startup: 2,
//!             active: 2,
//!             recovery: 6,
//!             motion: (root: [(at: 4, offset: (0.0, 0.0, 0.2))]),
//!         )],
//!     ),
//! )"#).unwrap();
//! let moves = sim.add_move_set(document.compile().unwrap());
//! let player = sim.spawn(ActorSpawn { moves: Some(moves), ..ActorSpawn::default() }).unwrap();
//!
//! let mut actions = TickActions::NEUTRAL;
//! actions
//!     .set(player, ActorActions { buttons: Buttons::LIGHT, stick: Stick::CENTER })
//!     .unwrap();
//! sim.step(&actions);
//! // The press lands on its own step, at the move's first tick.
//! assert_eq!(sim.actor(player).unwrap().moves.current.unwrap().tick(), 0);
//! sim.step(&TickActions::NEUTRAL);
//!
//! assert_eq!(sim.tick(), 2);
//! assert_eq!(sim.actor(player).unwrap().position.z, 0.05);
//! assert_ne!(sim.checksum(), 0);
//! ```

// The mesh arena in `tests/support` names this crate, and the controller's own tests share it.
#[cfg(test)]
extern crate self as sapphire_gameplay;

mod actions;
mod actors;
pub mod arena;
pub mod body;
pub mod brain;
mod checksum;
mod controller;
mod cues;
mod hits;
mod intents;
mod movement;
mod moves;
mod navigation;
mod party;
pub mod replay;
mod resolve;
mod simulation;
mod stats;
pub mod targeting;
mod timeline;

pub use actions::{ActionsError, ActorActions, Buttons, Controls, Stick, TickActions, TickActionsFull};
pub use actors::{ActorId, ActorRequests, ActorSpawn, ActorTable, ActorView, SpawnError};
pub use brain::{Brain, BrainState};
pub use checksum::StateHasher;
pub use controller::{
    CharacterController, CharacterWorld, ControllerDrive, ControllerEvents, ControllerInput,
    ControllerProfile, ControllerState, GroundMode, MovementMode, PhysicsWorldError, ProfileError, ProfileId,
    ResolvedProfile, actor_body, step_controller,
};
pub use cues::{
    CUE_QUEUE_LEN, CueEvent, CueQueue, DEFEATED, HitCue, PARTY_DEFEATED, PARTY_SWAP, PERFECT_DODGE,
    POISE_BROKEN,
};
pub use hits::{
    DodgedSwing, HitEvent, HitQuery, HitStats, HurtboxId, MAX_CAST_RESULTS, MAX_CONTACTS, MAX_DODGED_SWINGS,
    MAX_HIT_EVENTS, MAX_HIT_RECORDS, MAX_HURTBOXES, MAX_SAG, MAX_STOPPED_VOLUMES, MAX_SUB_STEPS,
    MIN_TURN_STEP, OwnerStep, PublishedMoves, SubStep, VolumeSweep, dodge_body, dodger_of, hit_shape,
    published_hurtbox, remove_hurtboxes, volume_pose,
};
pub use intents::{INTENT_RING_LEN, Intent, IntentRing};
pub use movement::{ActorTransform, RootDelta};
pub use moves::{
    ActiveHurtbox, ActiveVolume, CurrentMove, LiveWindows, MAX_ACTIVE_HURTBOXES, MAX_ACTIVE_VOLUMES,
    MoveLibrary, MoveOutput, MoveRead, MoveRef, MoveRunner, MoveSetId, Published, Tuned, WarpTarget,
};
pub use navigation::{PathId, SimNavigation};
pub use party::{PARTY_SLOTS, Party, PartyError, PartySlot, PartyTuning, SWAP_IN_MOVE, SWAP_INTENT};
pub use resolve::{Fighter, HitOutcome, SlowMotion};
pub use simulation::{CombatSet, SimConfig, SimRng, Simulation};
pub use stats::{
    BUDGET_DESKTOP_LOW, BUDGET_MOBILE_LOW, PHYSICS_BUDGET_DESKTOP_LOW, PHYSICS_BUDGET_MOBILE_LOW, SimStats,
};
pub use timeline::{
    TimelineCommand, TimelineEvent, TimelineId, TimelineLibrary, TimelinePlayError, TimelineProgress,
    TimelineRun,
};

#[doc(hidden)]
pub use hits::overhead::{
    OverheadAppliedContact, OverheadShadowBracket, OverheadShadowFailure, OverheadShadowResult,
    OverheadShadowShape, OverheadShadowUncertainty, OverheadShadowWitness, ShadowCapsule, ShadowEndpoint,
};
