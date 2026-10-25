//! Sapphire's renderer (spec §5.12). An anime character viewer built on the long-term
//! structure: M0's scene passes, M1's frame graph, HDR post-processing and local lights.
//!
//! # Owns
//!
//! - The extract boundary's data (§4.6): the plain-data [`FramePacket`] and the
//!   [`FramePacketRing`] it is reused from. The renderer never sees gameplay or world types
//!   (rule R4): time of day (`sapphire_world::tod`, M1 W1) reaches it only as the packet's
//!   resolved scene constants (key light, sky, post, and the ambient, fog, shadow tint,
//!   character key light and day/night factor its later packages read).
//! - Turning packets into pixels.
//! - The persistent GPU scene: a mesh pool (shared vertex-stream and index buffers), the
//!   material buffers, and instance and draw-record buffers kept across frames and updated from
//!   each packet's instance deltas. Only changed instances are uploaded, and the draw list is
//!   rebuilt only when the drawn set changes.
//! - Materials on the GPU (`materials`, M1 A3): cooked
//!   [`Material`](sapphire_assets::material::Material)s of every family in stable slots of their
//!   family's parameter buffer, each block copied as cooked, and their textures in group 3 (see
//!   "Material buffers" below). [`Renderer::update_material`] rewrites one slot: the path for
//!   hot reload and live link. The anime character family drives the anime character shader
//!   (`shaders/anime_character.wgsl`) and its outlines, which sample its textures through
//!   their cooked swizzles.
//! - Decoding cooked meshes on the GPU: the vertex shaders read the quantized streams of
//!   `sapphire_assets::mesh` as uploaded, byte for byte (see "Vertex streams" below).
//! - The scene pipelines: two-tone anime character opaque, inverted-hull outline, sky gradient.
//! - Local lights (`lights`, M1 R5): the packet's plain-data [`LocalLight`]s, culled to the
//!   view and capped, then listed for the anime character shader by clustered forward+ (a
//!   16 × 9 × 24 grid binned by a compute pass, desktop) or per-object lists (at most 4 per
//!   instance, chosen on the CPU, mobile), as [`RenderSettings::light_path`] says. Both paths
//!   run on every backend. The anime character shader lights by each local light in the sun's
//!   two-tone model.
//! - Post-processing (`post`, M1 R3): HDR scene color, bloom, the tone curve and grade baked
//!   into blended 32³ LUTs, and the fused composite into the target, driven by plain-data
//!   [`PostParams`].
//! - Compute skinning and morph targets (`skinning`, M1 R2): every instance of a skinned or
//!   morphed mesh draws from output streams of its own, which one dispatch a frame writes from
//!   the packet's poses ([`SkinnedInstance`]); see "Skinned and morphed meshes" below.
//! - The prepasses (M1 R2): a character-only depth prepass, and a scene prepass of depth, world
//!   normals and velocity, as frame-graph transients for later passes to read
//!   ([`PrepassTargets`]); the graph culls them while nothing does.
//! - The frame graph ([`graph`]): pass culling and ordering, pooled transient textures,
//!   parallel recording and the frame's one submit.
//! - Texture uploads ([`texture`], M1 A2): cooked KTX2 textures to sampled GPU textures with
//!   every mip level, after checking that the device supports their format.
//! - Quality tiers ([`quality`], M1 R6): the tier table (`tiers.ron`, a versioned RON document
//!   compiled in), selecting a tier from the device class, [`GpuCaps`](sapphire_rhi::GpuCaps)
//!   and RAM, the `r.*` overrides, and each frame's render scale with dynamic resolution. The
//!   scene renders at the render scale ([`Renderer::set_render_scale`]) and post's final pass
//!   upsamples it to the target (see "Render scale" below).
//!
//! # Does not own
//!
//! - Extraction: the app's systems in the ECS `Extract` schedule fill the packet, using
//!   change detection so that only changed instances and poses reach it. Lights are sent whole
//!   every frame.
//! - Animation: poses arrive as finished skinning palettes (`model_from_joint ×
//!   inverse_bind_matrix` per palette entry) and morph weights; sampling, blending and IK are
//!   `sapphire_anim`'s (spec §5.13).
//! - Knowing the device: the app gives [`quality`] a device profile (its class and RAM from the
//!   platform, its GPU from `GpuCaps`) and creates the renderer with the chosen tier's
//!   [`RenderSettings`]. The renderer never reads the platform (rule R7).
//! - Measuring frames: the app passes the frame graph's GPU timings and its own frame time to
//!   dynamic resolution ([`quality::FrameTiming`]), and turns GPU timing on while it runs.
//! - Devices, surfaces and presentation: `sapphire_rhi` and the app. Each frame the app builds
//!   a [`graph::FrameGraph`], the renderer adds its passes ([`Renderer::prepare`],
//!   [`Renderer::record`]), and the app runs it on the [`graph::GraphExecutor`] it keeps.
//! - Mesh, texture and material cooking: meshes arrive as cooked
//!   [`sapphire_assets::mesh::ArchivedMesh`]es, read in place, with outline normals, vertex
//!   order and LODs done by the pipeline; textures as [`sapphire_assets::texture::Texture`] with
//!   their mip chains, compressed for the device's cook; materials as packed GPU blocks with
//!   their texture references checked against the textures' classes.
//! - Resolving asset IDs: the app hands [`Renderer::upload_material`] the textures a material
//!   names, and [`Renderer::upload_mesh`] a material per mesh slot (the asset runtime, A4,
//!   takes this over).
//!
//! # Conventions
//!
//! - Right-handed world, Y up. Reverse-Z depth with an infinite far plane: depth 1 at the near
//!   plane, 0 at infinity, cleared to 0 and tested with `Greater` (better float precision).
//! - Shading is linear HDR light, unclamped, into `Rgba16Float` scene color. Post tone maps
//!   and grades it into the target, whose sRGB format encodes on write.
//! - Outline widths (`AnimeCharacterParams::edges.z`) are pixels at a 1080-pixel-high target.
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
//! | 1 | positions: unorm16 across the bounds, or f32 | 8 or 12 | every mesh pass, and alone by shadow passes and the character prepass |
//! | 2 | shading: octahedral normal and outline normal | 8 | surface shading, outline, scene prepass |
//! | 3 | surface: unorm16 UV, RGBA8 color | 8 | surface shading, outline (width scale) |
//! | 4 | previous positions: where the vertex was when last drawn, in slot 1's format | 8 or 12 | scene prepass (velocity) |
//!
//! The dequantization travels in the draw record rather than a mesh table, so decoding costs
//! no binding (see the budget below). Both position formats share the shaders: f32 positions
//! use an identity transform, and only the pipelines' slot 1 attribute differs. f32-position
//! pipelines are built when the first such mesh, or skinned or morphed mesh, is uploaded.
//!
//! A skinned or morphed instance binds slots 1, 2 and 4 to its skinning outputs and slot 3 to
//! its mesh's surface stream; a static mesh binds its slab's streams, slot 4 being slot 1 again.
//! Every pass decodes positions through `object_position` (`frame.wgsl`) and marks its clip
//! position invariant, so the prepasses' depth is the main pass's, bit for bit.
//!
//! # Skinned and morphed meshes (M1 R2)
//!
//! A mesh with a skin or morph targets uploads its streams, influences and morph deltas to the
//! pool's deformable buffer rather than a slab (`mesh_pool`), and each of its instances gets
//! output streams of its own (`skinning`):
//!
//! - **Poses** come in the packet as [`SkinnedInstance`]s: a palette of
//!   `model_from_joint(bind.joint) × bind.inverse_bind_matrix` per
//!   [`SkinBind`](sapphire_assets::mesh::SkinBind), in the mesh's object space, and morph
//!   weights. Instances that hold still send none and keep their last pose; one never posed
//!   draws in its bind pose with every weight 0.
//! - **The skinning pass** runs once a frame, before everything that draws, as one dispatch
//!   over every instance posed since the last recorded frame: it decodes the bind pose to f32,
//!   adds the morph deltas (regrouped by vertex at upload, so a vertex walks only its own),
//!   blends four palette matrices per vertex, and writes f32 positions and
//!   [`VertexShading`](sapphire_assets::mesh::VertexShading) normals. Only the drawn LOD's
//!   prefix of the streams is skinned (LOD0 until LOD selection).
//! - **Draws** read those outputs through the f32 pipelines with an identity dequantization
//!   (slots 1 and 2), and the mesh's surface stream (slot 3).
//! - **Velocity:** each instance's positions are double-buffered, so the scene prepass reads
//!   this frame's and the last drawn frame's (slot 4); instance transforms and the view keep
//!   their last drawn matrices too.
//!
//! Static meshes are untouched: they cost nothing extra and their draws are as before.
//!
//! # The prepasses (M1 R2)
//!
//! [`Renderer::record`] adds two prepasses after skinning and before the main pass, and returns
//! their targets ([`PrepassTargets`]): all single-sampled, at the scene's size (see "Render
//! scale"), sampled and copyable. Velocity is in texture coordinates, so it holds at any render
//! scale, and when the scale changes between frames.
//!
//! - **Character prepass** (every tier): depth of the skinned and morphed instances alone,
//!   position-only. Rim light, hair shadow on the face and eyes over the bangs read it (C2, C5).
//! - **Scene prepass**: depth, world normals (octahedral, `Rg16Float`) and velocity
//!   (`Rg16Float`, texture coordinates) of every opaque instance. Characters move by their
//!   skinned positions, static meshes by their instance transform, and both by the camera's.
//!   TAA (R7), outlines under TAA (P1), GTAO and screen-space edges read it.
//!
//! Nothing reads them yet, so the frame graph culls both passes: they cost two boxed closures
//! a frame and no GPU time until a consumer lands. The outline hull isn't in the scene prepass:
//! outlines v2 (C4) adds its velocity.
//!
//! # Render scale
//!
//! Every pass before post's final one renders at the scene's size, [`scaled_size`] of the target
//! at the render scale: the main pass's attachments and the prepass targets, the views (whose
//! viewport sizes outline widths and light-cluster tiles) and the bloom chain. Post's fused pass writes the target and
//! upsamples bilinearly where the scene is smaller; at scale 1 it loads texel for texel, so
//! frames are exactly what they were without a render scale. Passes added after the renderer's
//! (the debug overlay, UI) draw at the target's size. Each new size is one new set of transients
//! in the frame graph's pool, which is why dynamic resolution moves in steps.
//!
//! # Binding budget
//!
//! | Group | Binding | Stages | Contents | Written |
//! |---|---|---|---|---|
//! | 0 frame | 0: uniform | vertex, fragment | sun, sky, time, light counts, cluster depth slices, day/night | `prepare` |
//! | 0 frame | 1: storage | fragment | local lights (`LocalLight`, 64 bytes each) | `prepare`, when they changed |
//! | 0 frame | 2: storage | fragment | light lists: one per cluster, or one per instance slot | the cluster pass, or `prepare` for changed instance lists |
//! | 1 view | uniform, dynamic offset | vertex, fragment, compute | matrices (this frame's and the last drawn one's view-projection), camera, viewport | `record` |
//! | 2 scene | 0: storage | vertex | instance transforms (current and last drawn), by slot | `prepare`, changed instances only, and once more when they come to rest |
//! | 2 scene | 1: storage | vertex, fragment | the drawn family's material blocks, by slot | the uploads pass, changed slots only |
//! | 3 material | 0–7: textures | fragment | one material's texture slots, white 1×1 where unbound | material upload or update |
//! | 3 material | 8, 9: samplers | fragment | trilinear repeating; linear clamped for ramps | — |
//!
//! Vertex buffers: slot 0 is the draw records, slots 1–4 a mesh's streams (above). The
//! prepasses bind groups 0–2 like the main pass and read no storage buffer beyond the
//! instances.
//!
//! The skinning pass (`shaders/skinning.wgsl`) binds its own group 0, all compute:
//!
//! | Binding | Type | Contents | Written |
//! |---|---|---|---|
//! | 0 | storage | deformable meshes' streams, influences and regrouped morph deltas | mesh upload |
//! | 1 | storage | this frame's jobs, palettes (three rows each) and morph weights | `record`, through the staging belt |
//! | 2 | storage, read-write | every deformed instance's outputs: positions twice and normals | the skinning pass |
//!
//! Post passes bind their own two groups (`shaders/post.wgsl`): 0 holds the post constants
//! and a sampler, 1 the textures the pass reads. The light-cluster pass (`shaders/clusters.wgsl`)
//! binds its own group 0 (the frame constants, the lights read-only and the cluster lists
//! read-write: the same buffers) and the view group as group 1.
//!
//! Storage buffers per stage are budgeted against the 4-per-stage floor of Vulkan 1.1 phones
//! (see `sapphire_rhi`): vertex 2 (instances, materials), fragment 3 (materials, lights, light
//! lists), compute 2 in the cluster pass (lights, cluster lists) and 3 in the skinning pass
//! (sources, frame data, outputs). That leaves one spare in the fragment stage, two in the vertex
//! stage and one in the skinning pass. Group 3 is the last of the 4 bind groups the floor allows;
//! the prepasses, which read no material textures, leave it out. The anime character family
//! binds 7 of the 16 sampled textures and 2 of the 16 samplers a fragment stage gets, leaving 9
//! textures for frame-wide ones (shadow cascades, probes, the sky cubemap); the other families
//! bind 1 to 5. Local lights take only two fragment bindings because both light paths share one
//! list buffer layout, a count and then 16-bit light indices, read by cluster index or by
//! instance slot (passed from the vertex stage as a flat varying). Compute skinning writes
//! vertex buffers and adds nothing to the vertex stage, and packs a mesh's streams, skin and
//! morphs into one buffer, and a frame's jobs, palettes and weights into another, to stay within
//! its four. Draw records are vertex data rather than a third vertex-stage storage buffer for the
//! same reason.
//!
//! # Material buffers
//!
//! Each material family has a parameter buffer of its own, an array of its block
//! (`shaders/materials.wgsl`, mirroring `sapphire_assets::material`): 896 bytes for the anime
//! character family, 80 to 160 for the others. One shared array would pad every slot to the
//! largest block, and a pipeline only reads its own family, so each family's pipelines bind
//! their buffer as group 2 binding 1 and the storage-buffer budget above is unchanged. A draw
//! record's `material` is the slot in that buffer; a slot is kept for the renderer's lifetime
//! and written through a CPU mirror, so editing a material uploads its one block with the next
//! frame and nothing else. Every family uploads and binds its textures today; only the anime
//! character family has shaders, so [`Renderer::upload_mesh`] accepts only its materials.
//!
//! Material textures are sampled with explicit gradients (taken once per pixel, in uniform
//! control flow) so that unbound slots can be skipped on a branch, and every sample goes through
//! the slot's swizzle code from the block, which makes ASTC's two-channel textures read as
//! BC5's do.
//!
//! # Later milestones
//!
//! - **M1:** paged mesh residency with free lists and generational [`MeshHandle`]s; mesh
//!   uploads through a persistent staging ring in place of `Queue::write_buffer`;
//!   pipeline/material/mesh draw sorting. The [`FramePacket`] grows with the systems that
//!   feed it: VFX, UI and debug draws, plus shadow-cascade and UI-inset views.
//! - **Skinning** (see the `skinning` module): skinning only visible instances, at their
//!   selected LOD's prefix, with posed bounds from their joints; exact normals for
//!   non-uniformly scaled joints; morphed outline normals with C4's tangent-space outline
//!   normals; vertex pulling for multi-draw-indirect of skinned instances.
//! - **Prepasses:** their consumers (rim and hair shadow, TAA, outlines under TAA); the outline
//!   hull's velocity (C4); jitter-free velocity once TAA jitters the projection (R7); and, when
//!   TAA turns MSAA off, letting the main pass reuse the scene prepass's depth.
//! - **Local lights** (see the `lights` module): the light-cluster heatmap debug view, light
//!   layers, the character rig's ceiling and rim, local-light shadows, and two-level culling for
//!   hundreds of lights. Mesh bounds move from the per-object light lists into the mesh records
//!   once CPU culling needs them too.
//! - **LOD selection**, with culling: the pool holds every LOD's indices and ranges, and each
//!   cooked LOD carries its error in object-space meters, to project into pixels per view. Until
//!   then draws use LOD0.
//! - **Android pre-rotation:** whether the compositor rotates every frame has to be measured
//!   on a real phone first (spec open question Q14).
//! - **Upscaling** (spec §5.12, "Anti-aliasing and upscaling"): below render scale 1 the scene
//!   is upsampled bilinearly. TAA's own upsampling (R7), a spatial upscaler (an FSR1-style EASU
//!   and RCAS port, or MetalFX spatial) and DLSS or MetalFX temporal take over from it; the
//!   quality tiers' render scale and dynamic resolution then drive their input size.
//! - **Material families' shaders:** sky (W2), fog cards (W3), environment (W4) and foliage
//!   (W6) bring pipelines that bind their family's buffer (in a scene group of their own) and
//!   group 3. The anime character shader grows ramp and three-band diffuse, per-region reads of
//!   the control map (its alpha read without filtering, clamped to the region count), faces,
//!   hair and eyes (C1–C7). Alpha-tested and transparent materials draw opaque until C1's
//!   permutations. Outlines v2 (C4) adds the outline Z offset already in the block, skips the
//!   hull for materials without `MaterialFlags::OUTLINE`, and makes group 3 visible to the
//!   vertex stage if a width texture needs it.
//! - **Textures replaced at runtime** (hot reload, A8) rebind the materials that use them;
//!   anisotropic filtering joins the quality tiers.

mod buffer;
mod draw;
mod frame;
mod gpu_types;
pub mod graph;
mod lights;
mod materials;
mod mesh_pool;
mod pipelines;
mod post;
pub mod quality;
mod renderer;
mod scene;
mod shaders;
mod skinning;
pub mod texture;

pub use frame::{
    AmbientLight, CameraData, Celestial, CharacterLight, DirectionalLight, Fog, FogLayer, FramePacket,
    FramePacketRing, HeightFog, InstanceDeltas, InstanceId, LightKind, LocalLight, MeshInstance, PacketRange,
    SkinnedInstance, SkyGradient, ViewData,
};
pub use lights::{LightPath, LightStats, MAX_LIGHTS};
pub use materials::MaterialHandle;
pub use mesh_pool::MeshHandle;
pub use post::{BloomParams, ColorGrade, MAX_BLOOM_LEVELS, PostParams, ToneCurve, Vignette};
pub use renderer::{
    MIN_RENDER_SCALE, Msaa, PrepassTargets, RenderError, RenderSettings, RenderStats, Renderer, scaled_size,
};
pub use skinning::SkinningStats;

/// Log target for this crate.
pub(crate) const LOG_TARGET: &str = "sapphire::render";
