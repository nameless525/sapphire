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
//!   [`ImportedMesh`](mesh::ImportedMesh) with skins, morph targets, authored outline normals
//!   and anime character materials generated from the source's, or a
//!   [`SourceImage`](import::image::SourceImage) from PNG, TGA or EXR. VRM 1.0 and 0.x files
//!   ([`import::vrm`], M1 A6) import as a mesh whose MToon materials map onto anime character
//!   materials, and a [`CharacterRig`](sapphire_assets::rig::CharacterRig): humanoid,
//!   expressions, look-at, spring bones and constraints.
//! - [`extract`]: the `extract-materials` command, which writes a mesh's materials out as
//!   material documents bound in its sidecar, and a VRM's images as texture sources.
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
//! - [`material`]: material documents (`.material.ron`) cooked to
//!   [`Material`](sapphire_assets::material::Material)s, checked against the textures (and
//!   character maps) they reference, whose swizzles they record.
//! - [`cook`]: import, bake, optimize or compress, and validate for a target cook, written to
//!   disk through the content cache; a source can cook to sub-assets beside its own (a VRM's
//!   rig).
//! - [`pack`]: the last stage (M1 A4): a cook's outputs into packs, the files content ships in,
//!   with the grouping rules and the codec each kind is compressed with.
//! - [`cache`]: the cache key over everything that determines a cooked output or a pack, and the
//!   local store of cooked outputs and pack encodings by key.
//! - [`samples`]: procedurally generated source assets used to bring up the renderer: the
//!   mannequin, the shader swatches' maps and materials, the face test head with its face
//!   maps and material, the sky's maps and material, and the environment's courtyard with its
//!   environment materials and maps.
//!
//! # Does not own
//!
//! - The [`AssetId`] type, the cooked formats (`.smesh`, the KTX2 texture profile, `.smat`,
//!   `.srig`, what the character maps' texels mean, `.spack` packs) and the material documents
//!   and their GPU blocks: `sapphire_assets`, shared with the runtime.
//! - Simulating spring bones and evaluating constraints and expressions: animation
//!   (`sapphire_anim`, M2), from the rigs cooked here.
//! - Baking masks from a character in Blender: a script under `tools/blender/`, described
//!   in `docs/blender-mask-bake.md`.
//! - Document and archive formats: `sapphire_serialize` (sidecars are its RON documents).
//! - Locking and output locations: xtask (`cargo xtask content` cooks into
//!   `target/content/<cook>/` with the cache in `target/content/cache/`).
//!
//! # Cooked files and IDs
//!
//! Loose cooked files mirror their source paths (`samples/mannequin.glb` →
//! `samples/mannequin.smesh`): what tools read, and what hot reload (A8) will watch. The runtime
//! never sees them. Content ships as packs ([`pack`], M1 A4), whose tables of contents map each
//! [`AssetId`] to its data, so the runtime addresses assets by ID alone (rule R5). The packer
//! gets each asset's ID and output from the cook in process ([`cook::CookReport`]), so no
//! ID-to-path manifest exists anywhere. Sub-assets (a VRM's rig beside its mesh, `alicia.srig`)
//! are assets like any other there and in packs, with IDs derived from their source's
//! ([`meta::sub_asset_id`]).
//!
//! # Later milestones
//!
//! - FBX through `ufbx`.
//! - VRM: as the anime character family gains slots (C1–C7: a rim mask, emission, a shade map,
//!   UV animation, a cull mode), the MToon mapping fills them, and extraction binds the
//!   textures it already writes. `VRMC_springBone_extended_collider` and VRMA animations
//!   (`VRMC_vrm_animation`) with M2's animation.
//! - Skeletons as assets of their own, shared by meshes and animation clips (ozz archives,
//!   M2); today each skinned mesh embeds its skeleton, and a VRM's rig indexes its mesh's.
//! - Pack groups per zone, character and encounter once those exist as documents (see
//!   [`pack`]), a shared cache, and the editor's daemon mode.
//!
//! [`AssetId`]: sapphire_assets::AssetId

pub mod bake;
pub mod cache;
pub mod character_maps;
mod color;
pub mod cook;
pub mod extract;
mod gltf_writer;
pub mod import;
pub mod material;
pub mod mesh;
pub mod meta;
pub mod optimize;
pub mod pack;
pub mod samples;
#[cfg(test)]
mod test_util;
pub mod texture;
