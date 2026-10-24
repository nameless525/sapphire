//! Runtime asset formats and loading.
//!
//! M0 contains only the cooked mesh format ([`mesh`]), shared by the cooker
//! (`sapphire_pipeline`, which writes it) and the renderer (which reads it).
//! The M1 asset system (GUID handles, packs, async loading) grows here.

pub mod mesh;
