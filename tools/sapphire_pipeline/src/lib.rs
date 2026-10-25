//! Asset pipeline: sidecars, importers, bakers, cookers and the content cache. Never shipped.
//!
//! Turns authored source assets (under `content/`) into the cooked files the runtime loads
//! (spec §5.27). The runtime never links this crate (rule R1): it only reads cooked bytes,
//! and xtask runs the pipeline as a process.
//!
//! # Owns
//!
//! - [`meta`]: `.meta` sidecars, which give every source asset its [`AssetId`] and import
//!   settings, and the only place new IDs are made.
//! - [`import`]: source file → engine-neutral intermediate: a
//!   [`MeshAsset`](sapphire_assets::mesh::MeshAsset), or a [`SourceImage`](import::image::SourceImage)
//!   from PNG, TGA or EXR.
//! - [`bake`]: derived data computed offline, such as smoothed outline normals.
//! - [`texture`]: texture classes and their per-cook formats, mip chains, and compression
//!   through ctt into KTX2 [`Texture`](sapphire_assets::texture::Texture)s.
//! - [`cook`]: import + bake + validate for a target cook, written to disk through the
//!   content cache.
//! - [`cache`]: the cache key over everything that determines a cooked output, and the local
//!   store of cooked outputs by key.
//! - [`samples`]: procedurally generated source assets used to bring up the renderer.
//!
//! # Does not own
//!
//! - The [`AssetId`] type and the cooked formats (`.smesh`, the KTX2 texture profile):
//!   `sapphire_assets`, shared with the runtime.
//! - Document and archive formats: `sapphire_serialize` (sidecars are its RON documents).
//! - Locking and output locations: xtask (`cargo xtask content` cooks into
//!   `target/content/<cook>/` with the cache in `target/content/cache/`).
//!
//! # Cooked files and IDs
//!
//! Cooked files mirror their source paths (`samples/mannequin.glb` →
//! `samples/mannequin.smesh`), and the runtime still loads them by path. Decided in M1 A1:
//! cooked outputs become addressable by ID with packs (A4), whose table of contents maps each
//! [`AssetId`] to its data. Until then the pipeline writes no ID-to-path manifest: rule R5
//! keeps path strings out of the runtime, so A4 would replace it, and the packer is a
//! pipeline stage that gets each asset's ID and output from the cook in process
//! ([`cook::CookReport`]).
//!
//! # Later milestones
//!
//! - Materials (A3) and skinned meshes (A5) as further importers and recipes, FBX through
//!   `ufbx`, meshopt optimization and LODs.
//! - Packs (A4), a shared cache, and the editor's daemon mode.
//!
//! [`AssetId`]: sapphire_assets::AssetId

pub mod bake;
pub mod cache;
mod color;
pub mod cook;
mod gltf_writer;
pub mod import;
pub mod meta;
pub mod samples;
#[cfg(test)]
mod test_util;
pub mod texture;
