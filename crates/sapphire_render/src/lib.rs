//! Sapphire's renderer (spec §5.12). M0: a toon character viewer built on the long-term
//! structure.
//!
//! # Owns
//!
//! - Turning a plain-data [`FrameView`] into pixels. The renderer never sees gameplay or
//!   world types (rule R4); M1 replaces `FrameView` with the extracted `FramePacket` (§4.6).
//! - Persistent GPU scene buffers: a mesh pool (shared vertex and index buffers), a material
//!   buffer indexed by material ID, and per-frame instance and draw-record buffers.
//! - The toon pipelines: sky gradient, two-tone toon opaque, inverted-hull outline.
//!
//! # Does not own
//!
//! - Devices, surfaces and presentation: `sapphire_rhi`.
//! - Mesh cooking: meshes arrive as [`sapphire_assets::mesh::MeshAsset`] with smoothed
//!   outline normals already baked.
//!
//! # Submission paths
//!
//! Every draw reads its draw record (instance and material) through a draw ID delivered as an
//! instance-step vertex attribute, never through `instance_index`. That allows two
//! submission strategies over the same shaders:
//!
//! - **CPU-driven (M0, portable everywhere):** one `draw_indexed` per draw with
//!   `base_vertex = 0` (mesh-pool indices are absolute) and instances `0..1`, selecting the
//!   draw ID by vertex-buffer offset. It needs no base-vertex or first-instance support, so it
//!   runs on the iOS Simulator.
//! - **GPU-driven (later, desktop first):** GPU culling writes multi-draw-indirect records whose
//!   `first_instance` is the draw ID. Gated on `GpuCaps::multi_draw_indirect`.
//!
//! # Conventions
//!
//! - Right-handed world, Y up. Reverse-Z depth with an infinite far plane: depth 1 at the near
//!   plane, 0 at infinity, cleared to 0 and tested with `Greater` (better float precision).
//! - Shading is linear; the sRGB target format encodes on write. No HDR or tonemapping in M0.

mod buffer;
mod draw_ids;
mod frame;
mod gpu_types;
mod mesh_pool;
mod pipelines;
mod renderer;
mod scene;
mod shaders;

pub use frame::{CameraData, DirectionalLight, FrameView, MeshInstance, SkyGradient};
pub use mesh_pool::MeshHandle;
pub use renderer::{Msaa, RenderError, RenderSettings, Renderer};

/// Log target for this crate.
pub(crate) const LOG_TARGET: &str = "sapphire::render";
