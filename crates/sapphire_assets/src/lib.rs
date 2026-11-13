//! Runtime asset identity, formats and loading (spec §5.10).
//!
//! # Owns
//!
//! - Asset identity: [`AssetId`], the GUID cooked data refers to assets by (rule R5), re-exported
//!   from `sapphire_serialize`, beneath this crate because the plain-data formats it reads name
//!   assets too.
//! - Cooked formats shared by the cooker (`sapphire_pipeline`, which writes them) and the
//!   runtime (which reads them): [`mesh`], `.smesh` meshes with quantized vertex streams,
//!   LODs, skins, morph targets and material slots, read in place (M1 A5); [`texture`], KTX2
//!   files whose levels the renderer uploads as they are (M1 A2), cut into a tail and
//!   [`texture_level`]s (`.smip`) since M5 M3; and [`material`], `.smat`
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
//! - Skeletons and animation clips ([`skeleton`], `.sskel`, and [`clip`], `.sclip`, M2 N1): a
//!   character's joints as an asset that meshes and clips name, and motion as sparse tracks of
//!   quantized keys, read in place. Their formats sit here with every other cooked format's,
//!   because [`AssetKind::references`] checks each kind's bytes; `sapphire_anim` samples them.
//! - Scripts (`.sscript`, M3 L2): `sapphire_bytecode`'s `ScriptChunk`, loaded like any kind and read by
//!   whoever runs it, with no VM here.
//! - Dialogues (`.sdialogue`, M3 Q2): `sapphire_dialogue`'s `Dialogue`, loaded like any kind and walked by
//!   a game.
//! - Ambient probe grids ([`probes`], `.sprobe`, M1 W5): a zone's baked sky visibility and bounce
//!   light per time-of-day key, in a sparse grid of bricks, which the renderer relights.
//! - Where collider sets go (`.scol`, M2 D3): [`AssetKind::Colliders`] packs and loads them. Their
//!   format and checks are `sapphire_colliders`', which has no reason to know about packs.
//! - Where zones go (`.szone`, M3 W1): [`AssetKind::Zone`] packs and loads them and says what they
//!   reach ([`AssetKind::reach`]). Their format and checks are `sapphire_zone`'s.
//! - Fonts and font chains (M3 U3, spec §5.17): [`font`], fonts cut down for a pack with what their
//!   cook read, and which fonts answer for each use of type and script. Plain data as well, so the
//!   cooker writes them without linking a shaper (`sapphire_text`, which shapes with the bytes).
//! - What a mesh's material slots draw through, as data: [`PipelineKey`], which packs list per mesh
//!   (M3 K1) and the renderer derives its pipelines from, so the cooker needn't link the renderer.
//! - Where timelines go (`.stimeline`, M3 Q1): [`AssetKind::Timeline`] packs and loads them. Their format
//!   and checks are `sapphire_timeline`'s.
//! - Sounds and cues (M2 E1, spec §5.15): [`sound`], `.ssound` Ogg Vorbis files with what their cook
//!   learned, and the decoder cook and runtime share; [`cue`], the RON documents (cooked to `.scue`)
//!   that say how sounds play. Both are plain data, so the cooker reads and writes them without
//!   linking a mixer (`sapphire_audio`, which plays them). Music and voice (M3 A1) likewise: [`score`],
//!   stems by a fight's phase (`.sscore`), and [`voice`], one language's lines by key (`.svoice`). A voice
//!   sound's [`mouth`] curve (M5 C6) is cooked into its `.ssound`, for a face to say it with.
//! - Animation graphs (M2 N3, spec §5.13): [`graph`], the RON documents (cooked to `.sgraph`) that say
//!   how a character's clips play: which layers, which states, what plays for which move. Plain
//!   data, with its clips as dependencies; `sapphire_anim` runs it.
//! - Where string tables go (`.sloc`, M3 V2): [`AssetKind::Strings`] packs and loads them. Their format
//!   is `sapphire_strings`', and running them `sapphire_loc`'s, which has no reason to know about packs.
//! - Packs ([`pack`], M1 A4): the `.spack` files content ships in, cooked assets by ID in
//!   independently compressed 64 KB blocks (zstd or lz4) under blake3 hashes, with the reader,
//!   its integrity checks and the writer the packer uses. Each asset's [`AssetKind`] says how it
//!   is checked ([`AssetKind::references`]) and budgeted.
//! - Loading ([`AssetServer`], M1 A4): mounting packs, whose tables of contents are the runtime's
//!   only index from ID to data (rule R5); typed handles, strong ([`Handle`]) and weak
//!   ([`WeakHandle`]); the load pipeline (IO reads by priority, decode jobs on the job system's
//!   workers, dependencies first, a GPU upload queue under a per-frame budget); and residency
//!   (per-kind budgets, LRU eviction of unused assets, pinning). [`AssetServer`]'s docs describe
//!   each stage.
//! - The release manifest ([`manifest`], `release.smanifest`, M4 P1): one signed file naming every pack of a
//!   release by its size and header hash and every other shipped file by its blake3, and the shipping mount
//!   that refuses a pack it doesn't name ([`AssetServer::mount_only_released`]). Signing is the pipeline's;
//!   this crate only verifies.
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
//!   also samples clips, simulates spring bones and evaluates constraints and expressions from a
//!   rig.
//! - Checking that a material's textures exist and suit their slots: the pipeline, which knows
//!   texture classes. Binding textures and drawing materials: the renderer.
//!
//! # Later milestones
//!
//! - Group packs for characters (zones' came with M3 W1, and an encounter ships in its zone's): an asset is in one mounted
//!   pack only, so it moves between packs by a
//!   remount of the pack it left and then of the pack it moved to, in that order. (Live link
//!   needed no overlay mounts to push only what changed: its source serves the new pack whole,
//!   assembled on the device from the changed assets it received and the rest of the pack it had.)
//! - Letting packs a signed release manifest names skip bytecheck (spec §5.9).
//! - Memory-mapped packs, so raw blocks are read in place; cancelling the reads of assets dropped
//!   before they load; pinning by group (party, encounter) rather than by handle.
//! - Texture mip streaming (M5 M4, M5): a large texture is already a tail and levels, each an asset
//!   (M5 M3), but the server loads every level with whatever holds the texture
//!   ([`AssetServer`]'s "Texture levels"); raising and lowering levels by distance and budget replaces
//!   that.

pub mod character_maps;
pub mod clip;
pub mod cue;
pub mod documents;
pub mod font;
pub mod graph;
mod kind;
pub mod manifest;
pub mod material;
pub mod mesh;
pub mod mouth;
pub mod pack;
mod pipeline_key;
pub mod probes;
pub mod rig;
pub mod score;
mod server;
pub mod skeleton;
pub mod sound;
pub mod texture;
pub mod texture_level;
pub mod ui;
pub mod voice;

pub use kind::{AssetError, AssetKind};
pub use pipeline_key::{PipelineKey, SlotMaterial};
pub use sapphire_serialize::{AssetId, ParseAssetIdError};
pub use server::{
    Asset, AssetBudgets, AssetChange, AssetServer, AssetSlot, AssetStats, CpuResident, Handle, LoadError,
    LoadPriority, LoadState, MemoryPackSource, MountError, MountState, NamedAsset, PackFile, PackSource,
    ReadDone, RemountReport, TextureAsset, Unloaded, UnmountReport, UntypedHandle, Upload, UploadError,
    UploadReport, WeakHandle,
};
