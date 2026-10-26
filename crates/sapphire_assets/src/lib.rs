//! Runtime asset identity, formats and loading (spec §5.10).
//!
//! # Owns
//!
//! - Asset identity: [`AssetId`], the GUID cooked data refers to assets by (rule R5). Its
//!   text, serde and rkyv forms are defined here, because cooked data and documents both
//!   hold IDs.
//! - Cooked formats shared by the cooker (`sapphire_pipeline`, which writes them) and the
//!   runtime (which reads them): [`mesh`], `.smesh` meshes with quantized vertex streams,
//!   LODs, skins, morph targets and material slots, read in place (M1 A5); [`texture`], KTX2
//!   files whose levels the renderer uploads as they are (M1 A2); and [`material`], `.smat`
//!   materials holding their family's GPU parameter block and texture references (M1 A3). The
//!   mesh element encodings and the material blocks live here too, both directions, so cooker
//!   and runtime can't drift apart.
//! - The material documents: one RON document type per material family (anime character,
//!   environment, foliage, sky, fog card), and packing them into GPU blocks, which runtime
//!   tools (live link, the overlay) can do as well as the cooker.
//! - What the anime character maps' texels mean ([`character_maps`], M1 A7): the ILM channel
//!   assignment and material-ID and shadow-bias encodings, the ramp rows, and the SDF
//!   face-map angle encoding, for the same reason; and since M1 C3 the light in head space the
//!   face map is compared with, and the face mask and expression map's channels.
//! - Character rigs ([`rig`], `.srig`, M1 A6): a character's humanoid bone map, expressions
//!   as morph-target weights, look-at, spring bones and constraints, cooked from VRM files
//!   for animation (M2) to read.
//! - Packs ([`pack`], M1 A4): the `.spack` files content ships in, cooked assets by ID in
//!   independently compressed 64 KB blocks (zstd or lz4) under blake3 hashes, with the reader,
//!   its integrity checks and the writer the packer uses. Each asset's [`AssetKind`] says how it
//!   is checked ([`AssetKind::references`]) and budgeted.
//! - Loading ([`AssetServer`], M1 A4): mounting packs, whose tables of contents are the runtime's
//!   only index from ID to data (rule R5); typed handles, strong ([`Handle`]) and weak
//!   ([`WeakHandle`]); the load pipeline (IO reads by priority, decode jobs on the job system's
//!   workers, dependencies first, a GPU upload queue under a per-frame budget); and residency
//!   (per-kind budgets, LRU eviction of unused assets, pinning). The server's module docs
//!   describe each stage.
//! - Hot reload's runtime half (M1 A8): [`AssetServer::remount`] reads a changed pack again and
//!   reloads what its hashes say changed, through the same pipeline, swapping each new version in
//!   behind the asset's handles once it is uploaded. Every version an app is sent carries a
//!   [generation](Upload::generation), so it replaces its copy in place; materials are sent again
//!   when a texture they bind is replaced; [`AssetServer::generation`] and
//!   [`AssetServer::take_changes`] tell CPU systems what changed. A new version that fails keeps
//!   the old one. The triggers are elsewhere: the desktop's file watching
//!   (`sapphire_hot_reload`, dev builds) and live link (`sapphire_livelink`, M1 A9), which serves
//!   pushed packs through a [`PackSource`] of its own and remounts them.
//!
//! # Does not own
//!
//! - Creating IDs, `.meta` sidecars, import settings, the content cache and deciding what goes
//!   in which pack with which codec: the pipeline (tools). The runtime never generates an ID,
//!   so it never needs entropy.
//! - Importing, optimizing and simplifying meshes: the pipeline. [`mesh::Mesh::from_vertices`]
//!   packs procedural single-LOD meshes and nothing more.
//! - Data formats in general (RON documents, the rkyv cooked envelope): `sapphire_serialize`.
//! - Files and the IO thread: `sapphire_platform` (spec §4.3), reached through a
//!   [`PackSource`] the app provides, so this crate and the tools that use it never link SDL
//!   (ADR 0001). Threads for decoding: `sapphire_jobs`.
//! - Uploading: the app's uploader gets each asset as plain cooked data ([`Upload`]) and calls
//!   the renderer, which never sees handles or the server (rule R4).
//! - Watching files, or any other reason to remount: the app decides when a pack changed.
//! - Skinning and morphing: the renderer (M1 R2) and animation (`sapphire_anim`, M2), which
//!   also simulates spring bones and evaluates constraints and expressions from a rig.
//! - Checking that a material's textures exist and suit their slots: the pipeline, which knows
//!   texture classes. Binding textures and drawing materials: the renderer.
//!
//! # Later milestones
//!
//! - Assets moving from one mounted pack to another (group packs, M2): today the pack they left
//!   reports them removed, and the other's remount doesn't take them over. (Live link needed no
//!   overlay mounts to push only what changed: its source serves the new pack whole, assembled on
//!   the device from the changed assets it received and the rest of the pack it had.)
//! - Pack signatures checked against a signed manifest (`sapphire_patch`, M4), which lets
//!   verified packs skip bytecheck (spec §5.9); the header reserves their place.
//! - Memory-mapped packs, so raw blocks are read in place; unmounting packs at zone changes;
//!   cancelling the reads of assets dropped before they load; pinning by group (party,
//!   encounter) rather than by handle.
//! - Texture mip streaming within the upload budget, so a large texture uploads over several
//!   frames, lowest mips first: a [`texture::Texture`] already exposes each level on its own,
//!   smallest last in the list and first in the file.

pub mod character_maps;
mod id;
mod kind;
pub mod material;
pub mod mesh;
pub mod pack;
pub mod rig;
mod server;
pub mod texture;

pub use id::{AssetId, ParseAssetIdError};
pub use kind::{AssetError, AssetKind};
pub use server::{
    Asset, AssetBudgets, AssetChange, AssetServer, AssetSlot, AssetStats, Handle, LoadError, LoadPriority,
    LoadState, MemoryPackSource, MountError, PackFile, PackSource, ReadDone, RemountReport, TextureAsset,
    Unloaded, UntypedHandle, Upload, UploadError, UploadReport, WeakHandle,
};
