//! Runtime asset identity, formats and loading (spec §5.10).
//!
//! # Owns
//!
//! - Asset identity: [`AssetId`], the GUID cooked data refers to assets by (rule R5). Its
//!   text, serde and rkyv forms are defined here, because cooked data and documents both
//!   hold IDs.
//! - Cooked formats shared by the cooker (`sapphire_pipeline`, which writes them) and the
//!   runtime (which reads them): [`mesh`], `.smesh` meshes with quantized vertex streams,
//!   LODs, skins and morph targets, read in place (M1 A5), and [`texture`], KTX2 files whose
//!   levels the renderer uploads as they are (M1 A2). The mesh element encodings live here
//!   too, both directions, so cooker and runtime can't drift apart.
//! - What the anime character maps' texels mean ([`character_maps`], M1 A7): the ILM channel
//!   assignment and material-ID encoding, the ramp rows, and the SDF face-map angle encoding,
//!   for the same reason.
//!
//! # Does not own
//!
//! - Creating IDs, `.meta` sidecars, import settings and the content cache: the pipeline
//!   (tools). The runtime never generates an ID, so it never needs entropy.
//! - Importing, optimizing and simplifying meshes: the pipeline. [`mesh::Mesh::from_vertices`]
//!   packs procedural single-LOD meshes and nothing more.
//! - Data formats in general (RON documents, the rkyv cooked envelope): `sapphire_serialize`.
//! - Skinning and morphing: the renderer (M1 R2) and animation (`sapphire_anim`, M2).
//!
//! # Later milestones
//!
//! - **Addressing by ID (M1 A4).** Today the runtime still loads cooked files by path
//!   (the sandbox reads `samples/mannequin.smesh`). Resolving an [`AssetId`] is the pack
//!   table of contents' job (`AssetId → offset, size, codec, blake3`), together with
//!   handles, the IO thread, decode jobs and residency. A1 deliberately adds no interim
//!   ID-to-path manifest: rule R5 keeps path strings out of the runtime, and A4 would
//!   replace it.
//! - Hot reload behind handles (A8), and `Reflect` for [`AssetId`] once reflected documents
//!   hold asset references (materials, A3; the inspector, F5).
//! - Texture residency per tier budget, and mip streaming later still (spec §5.10): a
//!   [`texture::Texture`] already exposes each level on its own, smallest last in the list
//!   and first in the file.

pub mod character_maps;
mod id;
pub mod mesh;
pub mod texture;

pub use id::{AssetId, ParseAssetIdError};
