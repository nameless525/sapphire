//! Dialogue as plain data: what the cooker, the simulation and a game's panel agree on (spec §5.23, M3
//! Q2).
//!
//! A dialogue is a conversation as a graph: lines, choices and branches, with conditions on flags and
//! effects that set them. This crate holds the document, the cooked form and the walk through it, and
//! nothing that runs a script or draws, so the cooker can link it without a VM and a replay without a GPU.
//!
//! # Owns
//!
//! - **The document** ([`document`]): `.dialogue.ron`, [`DialogueDocument`], and its checks, which name the
//!   field.
//! - **Conditions** ([`condition`]): the small language a `when` is written in, and the Luau a cook makes
//!   of it, one function per condition in one chunk, for the script VM to run as hooks.
//! - **The flag registry** ([`flags`]): `content/flags.ron`, every flag a game has, at most [`MAX_FLAGS`].
//! - **The cooked format** ([`cooked`]): [`Dialogue`], an rkyv archive (`.sdialogue`), split into what the
//!   simulation reads ([`DialogueSimulation`]: effects) and what only presentation does
//!   ([`DialoguePresentation`]: the graph, its keys, the trigger that opens it, the conditions' source).
//! - **The walk** ([`walk`]): [`DialogueWalk`], where a conversation has got to, and [`DialogueView`], what
//!   a panel shows of it. [`Dialogue::line`] finds one line by its node's name, for a cutscene's subtitles.
//!
//! # Does not own
//!
//! - Flags themselves, and applying an effect: `sapphire_gameplay`'s `Flags`, in the simulation, hashed
//!   and saved, and an effect is a replay command there.
//! - Asking a condition: `sapphire_script`, which compiles the conditions' source, loads the chunk into a
//!   simulation's VM and runs a function between steps on a frame that can't write.
//! - Reading sources, checking keys against the tables and packs: `sapphire_pipeline`'s `dialogue` module.
//! - Drawing: a game's panel (`game_ui`'s dialogue screen) reads a [`DialogueView`].
//!
//! # Nothing here moves a checksum
//!
//! Walking a dialogue, viewing it and asking its conditions read the simulation and never write it. Only an
//! effect does, by setting flags, and a replay records that as it does any other command. So a run where
//! someone read every line and chose nothing hashes as a run with no dialogue at all.
//!
//! # Determinism
//!
//! What the simulation reads, an effect's writes, is names' hashes and whole numbers, hashed by value.
//! `[simulation]` in `xtask/layers.toml` holds this crate to the simulation's `clippy.toml`.

pub mod condition;
pub mod cooked;
pub mod document;
pub mod flags;
pub mod walk;

#[cfg(test)]
mod tests;

pub use condition::{Condition, MAX_CONDITION};
pub use cooked::{
    ArchivedDialogue, CookedArm, CookedChoice, CookedFace, CookedNode, CookedSay, DIALOGUE_EXTENSION,
    Dialogue, DialoguePresentation, DialogueSimulation, Effect, FlagWrite, LineView,
};
pub use document::{
    BranchArm, ChoiceDocument, DEFAULT_FACE_BLEND, DialogueDocument, DialogueError, KEY_PREFIX, MAX_ARMS,
    MAX_CHOICES, MAX_FACE_BLEND, MAX_FACES, MAX_NODES, MAX_SOURCE, MAX_WRITES, NodeDocument, SayDocument,
    SayFace,
};
pub use flags::{FlagRegistry, MAX_FLAGS};
pub use walk::{ChoiceView, ConditionId, DialogueView, DialogueWalk, DialogueWorld, EffectId};
