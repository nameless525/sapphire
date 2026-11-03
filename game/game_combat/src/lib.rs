//! A fight's app side: what stands a simulation on a stage and plays it there.
//!
//! The simulation is `sapphire_gameplay`'s and knows no renderer. This crate owns what a frame needs from a
//! fight: the arena and its camera ([`arena`]), the enemy and the blades ([`enemy`]), a fight's run and its file
//! ([`combat`]), the cues it fires ([`cues`]), the views that draw its volumes ([`combat_views`]), the party's
//! looks ([`party_looks`]), a timeline played over it (`timeline`), and live tuning in `dev` builds (`tuning`).
//!
//! [`fight::Fight`] holds it all, and it's a `sapphire_stage::SceneShow`: the caller owns it and passes it to
//! the scene's frame. A stage knows nothing of a fight.

// These docs are read with `--document-private-items`, as CI renders them.
#![allow(rustdoc::private_intra_doc_links)]

pub mod arena;
mod blade;
pub mod combat;
pub mod combat_views;
pub mod cues;
pub mod documents;
pub mod enemy;
pub mod fight;
pub mod party_looks;
mod timeline;
mod trails;
#[cfg(feature = "dev")]
pub mod tuning;

/// Log target for a fight.
pub(crate) const LOG_TARGET: &str = "game::combat";

/// mimalloc, counted, as an app's: tests hold allocations to what they should be.
#[cfg(test)]
#[global_allocator]
static GLOBAL: sapphire_core::alloc::EngineAllocator = sapphire_core::alloc::engine_allocator();
