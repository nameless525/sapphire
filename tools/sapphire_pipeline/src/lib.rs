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
//! - [`import`]: source file → engine-neutral intermediate: an
//!   [`ImportedMesh`](mesh::ImportedMesh) with skins, morph targets and authored outline
//!   normals, or a [`SourceImage`](import::image::SourceImage) from PNG, TGA or EXR.
//! - [`mesh`]: the mesh intermediate, and packing it into the cooked
//!   [`Mesh`](sapphire_assets::mesh::Mesh) (quantized streams).
//! - [`bake`]: derived data computed offline: smoothed outline normals where none are authored.
//! - [`optimize`]: meshoptimizer's vertex-cache, overdraw and vertex-fetch orders, and the LOD
//!   chain each cook keeps.
//! - [`texture`]: texture classes and their per-cook formats, mip chains, and compression
//!   through ctt into KTX2 [`Texture`](sapphire_assets::texture::Texture)s.
//! - [`character_maps`]: the anime character maps, generated from RON documents and cooked
//!   as textures: SDF face maps from threshold masks, ramps from color stops, and ILM maps
//!   packed from separate masks.
//! - [`cook`]: import, bake, optimize or compress, and validate for a target cook, written to
//!   disk through the content cache.
//! - [`cache`]: the cache key over everything that determines a cooked output, and the local
//!   store of cooked outputs by key.
//! - [`samples`]: procedurally generated source assets used to bring up the renderer.
//!
//! # Does not own
//!
//! - The [`AssetId`] type and the cooked formats (`.smesh`, the KTX2 texture profile, what
//!   the character maps' texels mean): `sapphire_assets`, shared with the runtime.
//! - Baking masks from a character in Blender: a script under `tools/blender/`, described
//!   in `docs/blender-mask-bake.md`.
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
//! - Materials (A3) as a further importer and recipe; FBX through `ufbx`.
//! - Skeletons as assets of their own, shared by meshes and animation clips (ozz archives,
//!   M2); today each skinned mesh embeds its skeleton, as it embeds its materials.
//! - Packs (A4), a shared cache, and the editor's daemon mode.
//!
//! [`AssetId`]: sapphire_assets::AssetId

pub mod bake;
pub mod cache;
pub mod character_maps;
mod color;
pub mod cook;
mod gltf_writer;
pub mod import;
pub mod mesh;
pub mod meta;
pub mod optimize;
pub mod samples;
#[cfg(test)]
mod test_util;
pub mod texture;
