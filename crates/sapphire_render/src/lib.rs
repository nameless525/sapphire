//! Sapphire's renderer (spec §5.12). An anime character viewer built on the long-term
//! structure: M0's scene passes, M1's frame graph, HDR post-processing and local lights.
//!
//! # Owns
//!
//! - The extract boundary's data (§4.6): the plain-data [`FramePacket`] and the
//!   [`FramePacketRing`] it is reused from. The renderer never sees gameplay or world types
//!   (rule R4): time of day (`sapphire_world::tod`, M1 W1) reaches it only as the packet's
//!   resolved scene constants (key light, sky gradient and layers, the sun, moon and celestial
//!   sphere, ambient, day/night factor and post, and the fog and shadow tint its later packages
//!   read), and the character light rig (`sapphire_world::light_rig`, M1 C2) only as each
//!   character's resolved [`CharacterLighting`].
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
//! - The scene pipelines: the anime character uber-shader and its inverted-hull outline, in
//!   one pipeline per static permutation (see "The anime character shader" below).
//! - The sky (`sky`, M1 W2): the gradient, stars and the Milky Way, the sun's and moon's discs and
//!   glows, cirrus and the card clouds' band, drawn at the far plane in the main pass from the
//!   packet's [`SkyLayers`] and its sky material, and a sky-only cubemap updated a face a frame
//!   for the probes and reflections that will read it (see "The sky" below).
//! - Local lights (`lights`, M1 R5): the packet's plain-data [`LocalLight`]s, culled to the
//!   view and capped, then listed for the anime character shader by clustered forward+ (a
//!   16 × 9 × 24 grid binned by a compute pass, desktop) or per-object lists (at most 4 per
//!   instance, chosen on the CPU, mobile), as [`RenderSettings::light_path`] says. Both paths
//!   run on every backend. The anime character shader lights by each local light in the key
//!   light's two-tone model.
//! - Characters' light rigs and faces on the GPU (`characters`, M1 C2 and C3): each frame's
//!   [`CharacterInstance`]s, one entry per character in a per-frame buffer that the anime
//!   character shader finds through the instance (see "The anime character shader" and "Faces"
//!   below).
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
//! - Loading and resolving assets: the asset server (`sapphire_assets`, M1 A4) loads a mesh's
//!   materials and their textures before the mesh and hands the app each as cooked data, within
//!   a per-frame upload budget. The app hands [`Renderer::upload_material`] the textures a
//!   material names and [`Renderer::upload_slot_materials`] the materials a mesh's slots name,
//!   so the renderer sees cooked data and its own handles, never asset handles (rule R4).
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
//! | 3 | surface: unorm16 UV, vertex color (AO, outline width, outline Z offset) and flags as unorm8 × 4 | 8 | surface shading, outline (width and Z offset) |
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
//! | 0 frame | 0: uniform | vertex, fragment | sun, sky, time, light counts, cluster depth slices, day/night, ambient | `prepare` |
//! | 0 frame | 1: storage | fragment | local lights (`LocalLight`, 64 bytes each) | `prepare`, when they changed |
//! | 0 frame | 2: storage | fragment | light lists: one per cluster, or one per instance slot | the cluster pass, or `prepare` for changed instance lists |
//! | 0 frame | 3: storage | fragment | per-character data (`CharacterData`): the world's key light, then the frame's characters' light rigs and faces, 128 bytes each | the uploads pass, when they changed |
//! | 1 view | uniform, dynamic offset | vertex, fragment, compute | matrices (this frame's and the last drawn one's view-projection), camera, viewport | `record` |
//! | 2 scene | 0: storage | vertex | instance transforms (current and last drawn) and character entries, by slot | `prepare`, changed instances only, and once more when they come to rest |
//! | 2 scene | 1: storage | vertex, fragment | the drawn family's material blocks, by slot | the uploads pass, changed slots only |
//! | 3 material | 0–7: textures | fragment | one material's texture slots (the anime character family's eight), white 1×1 where unbound | material upload or update |
//! | 3 material | 8–10: samplers | fragment | trilinear repeating; linear clamped for ramps; nearest for material IDs | — |
//!
//! Vertex buffers: slot 0 is the draw records, slots 1–4 a mesh's streams (above). The
//! prepasses bind groups 0–2 like the main pass and read no storage buffer beyond the
//! instances.
//!
//! The view group's buffer holds every view at a stride of the device's uniform offset
//! alignment: the main view (offset 0, written every frame), then the sky cube's six faces
//! (written once). The sky's pipelines bind groups 0 and 1 as the scene's, their own group 2 (the
//! frame's `SkyUniforms`, one uniform at binding 2, past the scene group's bindings
//! `frame.wgsl` declares) and the sky family's group 3 (five texture slots and the trilinear
//! sampler), so the sky reads no storage buffer beyond group 0's.
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
//! (see `sapphire_rhi`): vertex 2 (instances, materials), fragment 4 (materials, lights, light
//! lists, characters), compute 2 in the cluster pass (lights, cluster lists) and 3 in the
//! skinning pass (sources, frame data, outputs). That leaves none in the fragment stage, two in
//! the vertex stage and one in the skinning pass. The fragment stage's last went to per-character
//! data (M1 C2) because every later per-character need joins it rather than taking a binding:
//! faces' head basis and expressions (C3, beside the rig), the per-character shadow's matrices
//! (R4), outlines lit by the rig (C4). Frame-wide data that is not per character goes into
//! uniforms or textures (shadow cascades, probes). Group 3 is the last of the 4 bind groups the
//! floor allows; the prepasses, which read no material textures, leave it out. The anime
//! character family binds 8 of the 16 sampled textures and 3 of the 16 samplers a fragment stage
//! gets (trilinear, ramp, and nearest for material IDs), leaving 8 textures for frame-wide ones
//! (shadow cascades, probes, the sky cube); the other families bind 1 to 5. Local lights take
//! only two fragment bindings because both light paths share one list buffer layout, a count and
//! then 16-bit light indices, read by cluster index or by instance slot (passed from the vertex
//! stage as a flat varying). Compute skinning writes
//! vertex buffers and adds nothing to the vertex stage, and packs a mesh's streams, skin and
//! morphs into one buffer, and a frame's jobs, palettes and weights into another, to stay within
//! its four. Draw records are vertex data rather than a third vertex-stage storage buffer for the
//! same reason.
//!
//! # The anime character shader (M1 C1)
//!
//! `shaders/anime_character.wgsl` is one uber-shader for every anime character material (spec
//! §5.12): N·L, pushed toward shadow by vertex AO and the cast shadow and offset by the control
//! map's shadow bias, split at the material's threshold, with an authored shadow color from one
//! of two diffuse modes, ramp rows or two to three analytic bands, both per region. The control
//! map's material ID picks the region (read unfiltered from level 0), or the vertex's own
//! without a control map. The file's header has the details.
//!
//! - **Static permutations** are at most eight bits (`pipelines::Permutation`): of
//!   them, alpha test, transparency and faces are shader variants today, through
//!   pipeline-overridable constants, and each
//!   variant's pipelines are built when a mesh or a material edit first needs them. Everything
//!   else (ramp or bands, a deep band, which maps are bound) is a branch on the block that every
//!   pixel of a draw takes alike.
//! - **Draw order** within the main pass: opaque surfaces, alpha-tested ones, their outlines
//!   (only for materials with one), the sky, then transparent surfaces blended over it. Draws
//!   are grouped by permutation; transparent ones keep the draw list's order.
//! - **The frame** gives the day/night factor ramp mode blends day and night rows by
//!   ([`FramePacket::day_night`], from time of day), and the ambient light the GI reads
//!   ([`FramePacket::ambient`], a sky-and-ground hemisphere: `ambient_light` in `frame.wgsl`).
//! - **The character light rig (M1 C2)** lights each character in place of the world's sun and
//!   plain ambient: its own key light (the light split, the shadow side's tone and the rim) and
//!   stylized GI, resolved per character by the world and carried by the packet
//!   ([`FramePacket::characters`]). The fragment shader reads the instance's entry of the
//!   per-character buffer (group 0 binding 3; the `characters` module) once, through the
//!   `character` index the vertex stage passes on flat from the instance data, and every
//!   key-light term takes the key from there. Instances the packet doesn't list read entry 0:
//!   the world's key light ([`FramePacket::sun`]) with unstyled GI. The GI is the ambient
//!   hemisphere at a normal turned toward a sphere around the head (where the rig gives one) by
//!   the region's sphere weight and bent toward the camera, HSV-clamped, tinted on skin by the
//!   region's skin weight, and occluded by vertex AO ([`CharacterGi`]). Local lights keep their
//!   own directions and take the rig's luminance ceiling.
//! - **The vertex contract** is `sapphire_assets::mesh`'s: vertex color R ambient occlusion, G
//!   outline-width scale, B outline Z-offset scale, and a flags byte whose low bits are the
//!   vertex's material ID.
//! - **Cast shadows** enter through one function, `cast_shadow`, which R4's cascades fill; it
//!   returns 1 (lit) until then. Its contract with the light rig: it returns how much of the
//!   world's key light (the sun or the moon) reaches the point past environment casters alone,
//!   never the character's own geometry (characters don't self-shadow), however the rig turns
//!   the character's key; the split takes it as it takes vertex AO. The face permutation never
//!   calls it (see "Faces").
//!
//! # Faces (M1 C3)
//!
//! A face material (`CharacterSurface::Face`) draws through the face permutation
//! (`PERMUTATION_FACE`), whose light split comes from the face's SDF map and the character's key
//! light in head space instead of N·L (the shader's header and
//! `sapphire_assets::character_maps`, "Head space", give the rules):
//!
//! - **The head** is the character's: each [`CharacterInstance`] carries a [`CharacterFace`]
//!   beside its lighting, a [`HeadBasis`] in world space (the head bone's pose this frame) and
//!   [`FaceExpression`] weights, which the renderer packs beside the rig in the instance's
//!   per-character entry (WGSL `CharacterData::face`). The fragment shader reads it through the
//!   same index as the rig. An instance the packet doesn't list reads entry 0's: a head at rest,
//!   facing +Z with +Y up, with no expression.
//! - **The key light** is the rig's, read once with the body's as `key_direction` and
//!   `key_radiance`: face code reads no light of its own.
//! - **The split** reads the map at the pixel's `u` and mirrored, intersects the horizontal and
//!   vertical channels, and turns the lit margin into the N·L of a sphere whose terminator
//!   follows the map, so occlusion, the control map's bias and locks, the threshold and the
//!   diffuse mode apply as on the body: a face's shadow tones are the body's. So does the rig's
//!   stylized GI, which spherizes a face by its regions' sphere weight.
//! - **Masks:** always-lit eyes and brows, a face highlight, and a nose line that appears as the
//!   head turns from the camera (the face mask); blush and gloom scaled per character (the
//!   expression map).
//! - **No cast shadow:** shadow maps are too coarse and unstable at face scale, so the face
//!   permutation doesn't call `cast_shadow`, and R4 adds no shadow-map reads to it. An
//!   environment shadow reaches a face whole, per character: a value in the character's entry
//!   that the face takes as occlusion (R4), never shadow-map texels on it.
//!
//! # The sky (M1 W2)
//!
//! `shaders/sky.wgsl` draws the sky over a fullscreen triangle at the far plane, in the main pass
//! after the opaque surfaces and their outlines (depth test `Equal`, so only uncovered pixels
//! shade) and before transparent ones, and the same shader without the discs into the sky cube.
//! The `sky` module has the details:
//!
//! - **Layers**, back to front: the gradient (`sky_color()` in `frame.wgsl`, unchanged, which the
//!   anime character shader reads for ambient), stars and the Milky Way on the turning celestial
//!   sphere, the glows, the discs, cirrus and the card clouds. A layer at zero is skipped on a
//!   branch every pixel takes alike, and leaves the gradient exactly as it was.
//! - **Data:** time of day's [`SkyLayers`] (disc colors and glows, star brightness, cloud tones
//!   and coverage) and [`Celestial`] (where the bodies are, and the sphere's turn), and the
//!   packet's sky material ([`FramePacket::sky_material`]), or [`SkyMaterial::default`]
//!   (`sapphire_assets::material`), which the renderer uploads at creation. The CPU folds them,
//!   with the clouds' drift reduced in `f64` from the packet's unwrapped time, into one uniform
//!   block a frame.
//! - **The sky cube:** the sky without geometry, discs or stars, `Rg11b10Ufloat` (or
//!   `Rgba16Float`), one level, 256² on desktop, 128² on Mobile-High and 64² on Mobile-Low
//!   ([`SkySettings`] per tier), all six faces on the first frame and then a face every frame
//!   (every other one on Mobile-Low), in a side-effect pass before the main pass. Its readers are the ambient probes
//!   and reflections (W4, W5, C7).
//! - **Stats:** [`SkyStats`] in [`RenderStats::sky`]: the layers drawn, the cube's faces and
//!   sky materials that weren't.
//!
//! [`SkyMaterial::default`]: sapphire_assets::material::SkyMaterial
//!
//! # Material buffers
//!
//! Each material family has a parameter buffer of its own, an array of its block
//! (`shaders/materials.wgsl`, mirroring `sapphire_assets::material`): 896 bytes for the anime
//! character family, 80 to 160 for the others. One shared array would pad every slot to the
//! largest block, and a pipeline only reads its own family, so each family's pipelines bind
//! their buffer as group 2 binding 1 and the storage-buffer budget above is unchanged. The sky
//! draws one material at a time and reads its block from the CPU mirror into its frame uniforms
//! (see "The sky"), so the GPU copy of the sky family's buffer goes unread. A draw
//! record's `material` is the slot in that buffer; a slot is kept for the renderer's lifetime
//! and written through a CPU mirror, so editing a material uploads its one block with the next
//! frame and nothing else. Every family uploads and binds its textures today; meshes draw with
//! the anime character family alone, so [`Renderer::upload_mesh`] accepts only its materials, and
//! the sky draws with a sky material.
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
//!   layers (VFX lights kept off faces), a rim from the strongest local light (C5), local-light
//!   shadows, and two-level culling for hundreds of lights. Mesh bounds move from the
//!   per-object light lists into the mesh records once CPU culling needs them too.
//! - **LOD selection**, with culling: the pool holds every LOD's indices and ranges, and each
//!   cooked LOD carries its error in object-space meters, to project into pixels per view. Until
//!   then draws use LOD0.
//! - **Android pre-rotation:** whether the compositor rotates every frame has to be measured
//!   on a real phone first (spec open question Q14).
//! - **Upscaling** (spec §5.12, "Anti-aliasing and upscaling"): below render scale 1 the scene
//!   is upsampled bilinearly. TAA's own upsampling (R7), a spatial upscaler (an FSR1-style EASU
//!   and RCAS port, or MetalFX spatial) and DLSS or MetalFX temporal take over from it; the
//!   quality tiers' render scale and dynamic resolution then drive their input size.
//! - **Material families' shaders:** fog cards (W3), environment (W4) and foliage (W6) bring
//!   pipelines that bind their family's buffer (in a scene group of their own) and group 3. The
//!   sky (W2) reads its one material's block on the CPU instead (see "The sky").
//! - **The sky** (see the `sky` module's later milestones): weather, moon phases, cloud shadows,
//!   and the sky cube's readers (W4, W5) with its prefiltered mips.
//! - **The anime character shader** (see above) grows hair and eyes (C5, C6) with their
//!   permutation bits, and specular, matcaps and stockings (C7) over the control map channels it
//!   already reads. Shadows (R4) fill `cast_shadow`. Outlines v2 (C4) take each pixel's region
//!   from the control map as the surface does (today the vertex's), add the authored width curve
//!   and view-space flattening, and make group 3 visible to the vertex stage if a width texture
//!   needs it. Cooked MikkTSpace tangents, with C4's tangent-space outline normals, replace the
//!   normal map's screen-derivative frame.
//! - **Faces** (see above): hair shadow on the face and eyes over the bangs (C5); light layers,
//!   so local lights split on faces by the face map or not at all (today they split on the
//!   face's normals, as on the body); a face-SDF threshold debug view (F5's views); environment
//!   shadow reaching a face whole, per character (R4).
//! - **Permutations' remaining passes:** the prepasses cut alpha-tested materials out (a variant
//!   binding group 3) once something reads them; transparent surfaces sort back to front with
//!   draw sorting; alpha-to-coverage for alpha-tested surfaces under MSAA.
//! - **Textures replaced at runtime** (hot reload, A8) rebind the materials that use them;
//!   anisotropic filtering joins the quality tiers.

mod buffer;
mod characters;
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
mod sky;
pub mod texture;

pub use frame::{
    AmbientLight, CameraData, Celestial, CharacterFace, CharacterGi, CharacterInstance, CharacterLight,
    CharacterLighting, CloudLight, DirectionalLight, FaceExpression, Fog, FogLayer, FramePacket,
    FramePacketRing, HeadBasis, HeightFog, InstanceDeltas, InstanceId, LightKind, LocalLight, MeshInstance,
    PacketRange, SkinnedInstance, SkyDisc, SkyGradient, SkyLayers, ViewData,
};
pub use lights::{LightPath, LightStats, MAX_LIGHTS};
pub use materials::MaterialHandle;
pub use mesh_pool::MeshHandle;
pub use post::{BloomParams, ColorGrade, MAX_BLOOM_LEVELS, PostParams, ToneCurve, Vignette};
pub use renderer::{
    MIN_RENDER_SCALE, Msaa, PrepassTargets, RenderError, RenderSettings, RenderStats, Renderer, scaled_size,
};
pub use skinning::SkinningStats;
pub use sky::{SkySettings, SkyStats};

/// Log target for this crate.
pub(crate) const LOG_TARGET: &str = "sapphire::render";
