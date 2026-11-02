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
//!
//! The controller, moves, hits and cameras arrive with M2's G packages. Until then the crate's
//! systems are a drill (`drill`) that exercises the rest: a button makes an actor follow a baked
//! table on its own clock, others freeze it, slow it or spawn another actor.
//!
//! Doesn't own, and may never link: the renderer, the world (`sapphire_world`), animation, the
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
//! [`FixedPhysics`](sapphire_ecs::FixedPhysics) is empty until physics lands;
//! [`FixedPost`](sapphire_ecs::FixedPost) resolves the spawns and despawns queued during the step
//! (rule R3).
//!
//! # Example
//!
//! ```
//! use sapphire_gameplay::{ActorActions, ActorSpawn, Buttons, SimConfig, Simulation, Stick, TickActions};
//! use sapphire_moves::{BakedTables, RootSample};
//!
//! let mut sim = Simulation::new(SimConfig { seed: 7, ..SimConfig::default() });
//!
//! // One tick's walk forward, as a cook would bake it.
//! let step = RootSample { translation: [0.0, 0.0, 0.05], yaw: 0.0 };
//! let walk = sim.add_tables(BakedTables::new(vec![RootSample::ZERO, step], vec![]).unwrap());
//! let player = sim.spawn(ActorSpawn { table: Some(walk), ..ActorSpawn::default() }).unwrap();
//!
//! let mut actions = TickActions::NEUTRAL;
//! actions
//!     .set(player, ActorActions { buttons: Buttons::LIGHT, stick: Stick::CENTER })
//!     .unwrap();
//! sim.step(&actions);
//! sim.step(&TickActions::NEUTRAL);
//!
//! assert_eq!(sim.tick(), 2);
//! assert_eq!(sim.actor(player).unwrap().position.z, 0.05);
//! assert_ne!(sim.checksum(), 0);
//! ```

mod actions;
mod actors;
mod checksum;
mod drill;
mod motion;
pub mod replay;
mod simulation;
mod stats;
pub mod targeting;

pub use actions::{ActionsError, ActorActions, Buttons, Controls, Stick, TickActions, TickActionsFull};
pub use actors::{ActorId, ActorRequests, ActorSpawn, ActorTable, ActorView, SpawnError};
pub use checksum::StateHasher;
pub use drill::{DRILL_HITSTOP_TICKS, DRILL_LOCAL_SCALE, DRILL_WALK_SPEED, DRILL_WORLD_SCALE};
pub use motion::{ActorTransform, RootMotion, TableId, TableLibrary};
pub use simulation::{CombatSet, SimConfig, SimRng, Simulation};
pub use stats::{BUDGET_DESKTOP_LOW, BUDGET_MOBILE_LOW, SimStats};
