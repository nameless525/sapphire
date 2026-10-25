//! Sapphire's renderer (spec §5.12). A toon character viewer built on the long-term
//! structure: M0's scene passes, M1's frame graph, HDR post-processing and local lights.
//!
//! # Owns
//!
//! - The extract boundary's data (§4.6): the plain-data [`FramePacket`] and the
//!   [`FramePacketRing`] it is reused from. The renderer never sees gameplay or world types
//!   (rule R4).
//! - Turning packets into pixels.
//! - The persistent GPU scene: a mesh pool (shared vertex-stream and index buffers), a
//!   material buffer indexed by material ID, and instance and draw-record buffers kept across
//!   frames and updated from each packet's instance deltas. Only changed instances are
//!   uploaded, and the draw list is rebuilt only when the drawn set changes.
//! - Decoding cooked meshes on the GPU: the vertex shaders read the quantized streams of
//!   `sapphire_assets::mesh` as uploaded, byte for byte (see "Vertex streams" below).
//! - The toon pipelines: two-tone toon opaque, inverted-hull outline, sky gradient.
//! - Local lights (`lights`, M1 R5): the packet's plain-data [`LocalLight`]s, culled to the
//!   view and capped, then listed for the toon shader by clustered forward+ (a 16 × 9 × 24
//!   grid binned by a compute pass, desktop) or per-object lists (at most 4 per instance,
//!   chosen on the CPU, mobile), as [`RenderSettings::light_path`] says. Both paths run on
//!   every backend. The toon shader lights by each local light in the sun's toon model.
//! - Post-processing (`post`, M1 R3): HDR scene color, bloom, the tone curve and grade baked
//!   into blended 32³ LUTs, and the fused composite into the target, driven by plain-data
//!   [`PostParams`].
//! - The frame graph ([`graph`]): pass culling and ordering, pooled transient textures,
//!   parallel recording and the frame's one submit.
//! - Texture uploads ([`texture`], M1 A2): cooked KTX2 textures to sampled GPU textures with
//!   every mip level, after checking that the device supports their format.
//!
//! # Does not own
//!
//! - Extraction: the app's systems in the ECS `Extract` schedule fill the packet, using
//!   change detection so that only changed instances reach it. Lights are sent whole every
//!   frame.
//! - Choosing settings per device: the app passes [`RenderSettings`]
//!   ([`RenderSettings::for_device`] until the quality tiers of R6). The renderer never reads
//!   the platform (rule R7).
//! - Devices, surfaces and presentation: `sapphire_rhi` and the app. Each frame the app builds
//!   a [`graph::FrameGraph`], the renderer adds its passes ([`Renderer::prepare`],
//!   [`Renderer::record`]), and the app runs it on the [`graph::GraphExecutor`] it keeps.
//! - Mesh and texture cooking: meshes arrive as cooked [`sapphire_assets::mesh::ArchivedMesh`]es,
//!   read in place, with outline normals, vertex order and LODs done by the pipeline; textures
//!   as [`sapphire_assets::texture::Texture`] with their mip chains, compressed for the
//!   device's cook.
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
//!   `base_vertex = 0` and instances `0..1`. The mesh's streams and the draw record are
//!   selected by vertex-buffer offsets. It needs no base-vertex or first-instance support, so
//!   it runs on the iOS Simulator.
//! - **GPU-driven (later, desktop first):** GPU culling writes multi-draw-indirect records
//!   whose `base_vertex` is the mesh's first vertex in its slab and whose `first_instance` is
//!   the draw record. One indirect batch per position format, since each format has its own
//!   slab and pipelines. Gated on `GpuCaps::multi_draw_indirect`.
//!
//! # Vertex streams
//!
//! A cooked mesh's vertices are three streams, uploaded as cooked and decoded in the vertex
//! shaders (`decode_vertex` in `shaders/frame.wgsl`):
//!
//! | Slot | Stream | Bytes | Read by |
//! |---|---|---|---|
//! | 0 | draw record: instance, material, the mesh's dequantization | 48, per instance | every mesh pass |
//! | 1 | positions: unorm16 across the bounds, or f32 | 8 or 12 | every mesh pass, and alone by shadow and depth passes |
//! | 2 | shading: octahedral normal and outline normal | 8 | toon, outline |
//! | 3 | surface: unorm16 UV, RGBA8 color | 8 | toon, outline (width scale) |
//!
//! The dequantization travels in the draw record rather than a mesh table, so decoding costs
//! no binding (see the budget below). Both position formats share the shaders: f32 positions
//! use an identity transform, and only the pipelines' slot 1 attribute differs. f32-position
//! pipelines are built when the first such mesh is uploaded.
//!
//! # Skinned meshes (M1 A5 → R2)
//!
//! Skinned meshes upload their streams like any other and draw in **bind pose**, morph
//! targets at weight 0; their skins and morph targets stay in the cooked mesh. Compute
//! skinning (M1 R2) builds on this:
//!
//! - Upload [`Skin::influences`](sapphire_assets::mesh::Skin) (u8 or u16 palette indices plus
//!   unorm8 weights, 8 or 12 bytes a vertex) and the morph deltas (16 bytes each: vertex,
//!   binary16 position and normal deltas, sparse per target) as storage buffers, byte for byte.
//! - Palettes are `model_from_joint(bind.joint) × bind.inverse_bind_matrix` per
//!   [`SkinBind`](sapphire_assets::mesh::SkinBind); joints are parents-first with names and
//!   rest poses, the shape ozz-animation's skeleton takes (spec §5.13).
//! - Decode positions to f32 before adding deltas (morphed and skinned positions leave the
//!   bind-pose bounds), and write per-instance positions as f32 (slot 1, drawn with the f32
//!   pipelines and an identity dequantization in the draw record) and shading in the
//!   [`VertexShading`](sapphire_assets::mesh::VertexShading) layout (slot 2). The surface stream
//!   stays shared in the pool.
//! - Every LOD reads a prefix of the streams, so a distant character's dispatch skins only
//!   `lods[n].vertex_count` vertices.
//!
//! # Binding budget
//!
//! | Group | Binding | Stages | Contents | Written |
//! |---|---|---|---|---|
//! | 0 frame | 0: uniform | vertex, fragment | sun, sky, time, light counts, cluster depth slices | `prepare` |
//! | 0 frame | 1: storage | fragment | local lights (`LocalLight`, 64 bytes each) | `prepare`, when they changed |
//! | 0 frame | 2: storage | fragment | light lists: one per cluster, or one per instance slot | the cluster pass, or `prepare` for changed instance lists |
//! | 1 view | uniform, dynamic offset | vertex, fragment, compute | matrices, camera, viewport | `record` |
//! | 2 scene | 0: storage | vertex | instance transforms, by slot | `prepare`, changed instances only |
//! | 2 scene | 1: storage | vertex, fragment | toon materials | mesh upload |
//! | 3 | — | — | material textures (M1) | — |
//!
//! Vertex buffers: slot 0 is the draw records, slots 1–3 a mesh's streams (above).
//!
//! Post passes bind their own two groups (`shaders/post.wgsl`): 0 holds the post constants
//! and a sampler, 1 the textures the pass reads. The light-cluster pass (`shaders/clusters.wgsl`)
//! binds its own group 0 (the frame constants, the lights read-only and the cluster lists
//! read-write: the same buffers) and the view group as group 1.
//!
//! Storage buffers per stage are budgeted against the 4-per-stage floor of Vulkan 1.1 phones
//! (see `sapphire_rhi`): vertex 2 (instances, materials), fragment 3 (materials, lights, light
//! lists), compute 2 (lights, cluster lists). That leaves one spare in the fragment stage and two
//! in the vertex stage. Local lights take only two fragment bindings because both light paths
//! share one list buffer layout, a count and then 16-bit light indices, read by cluster index or
//! by instance slot (passed from the vertex stage as a flat varying). Compute skinning writes
//! vertex buffers and adds nothing to the vertex stage. Draw records are vertex data rather than
//! a third vertex-stage storage buffer for this reason.
//!
//! # Later milestones
//!
//! - **M1:** paged mesh residency with free lists and generational [`MeshHandle`]s; mesh
//!   uploads through a persistent staging ring in place of `Queue::write_buffer`;
//!   pipeline/material/mesh draw sorting. The [`FramePacket`] grows with the systems that
//!   feed it: skinned instances, morph weights, VFX, UI and debug draws, plus shadow-cascade
//!   and UI-inset views.
//! - **Local lights** (see the `lights` module): the light-cluster heatmap debug view, light
//!   layers, the character rig's ceiling and rim, local-light shadows, and two-level culling for
//!   hundreds of lights. Mesh bounds move from the per-object light lists into the mesh records
//!   once CPU culling needs them too.
//! - **LOD selection**, with culling: the pool holds every LOD's indices and ranges, and each
//!   cooked LOD carries its error in object-space meters, to project into pixels per view. Until
//!   then draws use LOD0.
//! - **Android pre-rotation:** whether the compositor rotates every frame has to be measured
//!   on a real phone first (spec open question Q14).

mod buffer;
mod frame;
mod gpu_types;
pub mod graph;
mod lights;
mod mesh_pool;
mod pipelines;
mod post;
mod renderer;
mod scene;
mod shaders;
pub mod texture;

pub use frame::{
    CameraData, DirectionalLight, FramePacket, FramePacketRing, InstanceDeltas, InstanceId, LightKind,
    LocalLight, MeshInstance, SkyGradient, ViewData,
};
pub use lights::{LightPath, LightStats, MAX_LIGHTS};
pub use mesh_pool::MeshHandle;
pub use post::{BloomParams, ColorGrade, MAX_BLOOM_LEVELS, PostParams, ToneCurve, Vignette};
pub use renderer::{Msaa, RenderError, RenderSettings, RenderStats, Renderer};

/// Log target for this crate.
pub(crate) const LOG_TARGET: &str = "sapphire::render";
