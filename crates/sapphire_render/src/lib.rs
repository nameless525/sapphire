//! Sapphire's renderer (spec §5.12). A toon character viewer built on the long-term
//! structure: M0's scene passes, M1's frame graph and HDR post-processing.
//!
//! # Owns
//!
//! - The extract boundary's data (§4.6): the plain-data [`FramePacket`] and the
//!   [`FramePacketRing`] it is reused from. The renderer never sees gameplay or world types
//!   (rule R4).
//! - Turning packets into pixels.
//! - The persistent GPU scene: a mesh pool (shared vertex and index buffers), a material
//!   buffer indexed by material ID, and instance and draw-record buffers kept across frames
//!   and updated from each packet's instance deltas. Only changed instances are uploaded, and
//!   the draw list is rebuilt only when the drawn set changes.
//! - The toon pipelines: two-tone toon opaque, inverted-hull outline, sky gradient.
//! - Post-processing (`post`, M1 R3): HDR scene color, bloom, the tone curve and grade baked
//!   into blended 32³ LUTs, and the fused composite into the target, driven by plain-data
//!   [`PostParams`].
//! - The frame graph ([`graph`]): pass culling and ordering, pooled transient textures,
//!   parallel recording and the frame's one submit.
//!
//! # Does not own
//!
//! - Extraction: the app's systems in the ECS `Extract` schedule fill the packet, using
//!   change detection so that only changed instances reach it.
//! - Devices, surfaces and presentation: `sapphire_rhi` and the app. Each frame the app builds
//!   a [`graph::FrameGraph`], the renderer adds its passes ([`Renderer::prepare`],
//!   [`Renderer::record`]), and the app runs it on the [`graph::GraphExecutor`] it keeps.
//! - Mesh cooking: meshes arrive as [`sapphire_assets::mesh::MeshAsset`] with smoothed
//!   outline normals already baked.
//!
//! # Conventions
//!
//! - Right-handed world, Y up. Reverse-Z depth with an infinite far plane: depth 1 at the near
//!   plane, 0 at infinity, cleared to 0 and tested with `Greater` (better float precision).
//! - Shading is linear HDR light, unclamped, into `Rgba16Float` scene color. Post tone maps
//!   and grades it into the target, whose sRGB format encodes on write.
//! - Outline widths (`ToonMaterialParams::params.w`) are pixels at a 1080-pixel-high target.
//!
//! # Submission paths
//!
//! Mesh indices are mesh-relative, and every draw reads its draw record (instance and
//! material) as an instance-step vertex attribute, never through `instance_index`. The same
//! shaders therefore serve two submission strategies:
//!
//! - **CPU-driven (M0, portable everywhere):** one `draw_indexed` per draw with
//!   `base_vertex = 0` and instances `0..1`. The mesh's vertices and the draw record are
//!   selected by vertex-buffer offsets. It needs no base-vertex or first-instance support, so
//!   it runs on the iOS Simulator.
//! - **GPU-driven (later, desktop first):** GPU culling writes multi-draw-indirect records
//!   whose `base_vertex` is the mesh's first vertex and whose `first_instance` is the draw
//!   record. Gated on `GpuCaps::multi_draw_indirect`.
//!
//! # Binding budget
//!
//! | Group | Binding | Stages | Contents | Written |
//! |---|---|---|---|---|
//! | 0 frame | uniform | vertex, fragment | sun, sky, time | `prepare` |
//! | 1 view | uniform, dynamic offset | vertex, fragment | matrices, camera, viewport | `record` |
//! | 2 scene | 0: storage | vertex | instance transforms, by slot | `prepare`, changed instances only |
//! | 2 scene | 1: storage | vertex, fragment | toon materials | mesh upload |
//! | 3 | — | — | material textures (M1) | — |
//!
//! Vertex buffers: slot 0 is the mesh pool, slot 1 the draw records.
//!
//! Post passes bind their own two groups (`shaders/post.wgsl`): 0 holds the post constants
//! and a sampler, 1 the textures the pass reads.
//!
//! Storage buffers per stage are budgeted against the 4-per-stage floor of Vulkan 1.1 phones
//! (see `sapphire_rhi`): vertex 2, fragment 1. M1 clustered lighting adds a light buffer and
//! one cluster buffer (grid and index lists packed together) to the fragment stage, which
//! leaves one spare per stage. Compute skinning writes vertex buffers and adds nothing to the
//! vertex stage. Draw records are vertex data rather than a third vertex-stage storage buffer
//! for this reason.
//!
//! # Later milestones
//!
//! - **M1:** paged mesh residency with free lists and generational [`MeshHandle`]s; mesh
//!   uploads through a persistent staging ring in place of `Queue::write_buffer`;
//!   pipeline/material/mesh draw sorting. The [`FramePacket`] grows with the systems that
//!   feed it: skinned instances, morph weights, lights, VFX, post parameters, UI and debug
//!   draws, plus shadow-cascade and UI-inset views.
//! - **Android pre-rotation:** whether the compositor rotates every frame has to be measured
//!   on a real phone first (spec open question Q14).

mod buffer;
mod frame;
mod gpu_types;
pub mod graph;
mod mesh_pool;
mod pipelines;
mod post;
mod renderer;
mod scene;
mod shaders;

pub use frame::{
    CameraData, DirectionalLight, FramePacket, FramePacketRing, InstanceDeltas, InstanceId, MeshInstance,
    SkyGradient, ViewData,
};
pub use mesh_pool::MeshHandle;
pub use post::{BloomParams, ColorGrade, MAX_BLOOM_LEVELS, PostParams, ToneCurve, Vignette};
pub use renderer::{Msaa, RenderError, RenderSettings, RenderStats, Renderer};

/// Log target for this crate.
pub(crate) const LOG_TARGET: &str = "sapphire::render";
