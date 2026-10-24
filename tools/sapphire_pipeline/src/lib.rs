//! Asset pipeline: importers, bakers and cookers. Never shipped.
//!
//! Owns turning authored source assets (under `content/`) into the cooked formats the
//! runtime loads (spec §5.27). The stages are:
//!
//! - [`import`]: source file → engine-neutral [`MeshAsset`](sapphire_assets::mesh::MeshAsset).
//! - [`bake`]: derived data computed offline, such as smoothed outline normals.
//! - [`cook`]: import + bake + validate for a target cook, written to disk.
//! - [`samples`]: procedurally generated source assets used to bring up the renderer.
//!
//! M0 deliberately does not have the content-hash cache, `.meta` GUID sidecars, packing,
//! meshopt optimization or LODs; those arrive in M1. The runtime never links this crate
//! (rule R1): it only reads the cooked bytes.

pub mod bake;
mod color;
pub mod cook;
mod gltf_writer;
pub mod import;
pub mod samples;
#[cfg(test)]
mod test_util;
