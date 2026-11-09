//! The game's interface: a fight's HUD and its touch zones, over `sapphire_ui`'s quads and their atlas.
//!
//! [`hud::ShownFight`] is a fight with its HUD on it, as a `sapphire_stage::SceneShow`, and [`menus::GameMenus`]
//! the title, the pause and the rest, on documents, over it. [`dialogue::DialoguePanel`] puts the dialogue
//! screen up on a fight's conversation.

// These docs are read with `--document-private-items`, as CI renders them.
#![allow(rustdoc::private_intra_doc_links)]

pub mod dialogue;
pub mod hud;
pub mod menus;
pub mod screen;
pub mod touch;
pub mod wipe;

/// Log target for the interface.
pub(crate) const LOG_TARGET: &str = "game::ui";

/// mimalloc, counted, as an app's: tests hold allocations to what they should be.
#[cfg(test)]
#[global_allocator]
static GLOBAL: sapphire_core::alloc::EngineAllocator = sapphire_core::alloc::engine_allocator();
