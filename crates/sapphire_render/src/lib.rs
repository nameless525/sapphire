//! Sapphire's renderer (spec §5.12). Anime characters in a stylized world, built on the
//! long-term structure: M0's scene passes, M1's frame graph, HDR post-processing and local lights.
//!
//! # Owns
//!
//! - The extract boundary's data (§4.6): the plain-data [`FramePacket`] and the
//!   [`FramePacketRing`] it is reused from. The renderer never sees gameplay or world types
//!   (rule R4): time of day (`sapphire_world::tod`, M1 W1) reaches it only as the packet's
//!   resolved scene constants (key light, sky gradient and layers, the sun, moon and celestial
//!   sphere, ambient, shadow tint, day/night factor, post with its compositing layers, and fog),
//!   and the character light rig (`sapphire_world::light_rig`, M1 C2) only as each character's
//!   resolved [`CharacterLighting`].
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
//!   (`shaders/anime_character.wgsl`) and its outlines, the environment family the environment
//!   shader (`shaders/environment.wgsl`, M1 W4), and the foliage family the foliage shader
//!   (`shaders/foliage.wgsl`, M1 W6); all sample their textures through their cooked swizzles.
//! - Decoding cooked meshes on the GPU: the vertex shaders read the quantized streams of
//!   `sapphire_assets::mesh` as uploaded, byte for byte (see "Vertex streams" below).
//! - The scene pipelines: the anime character uber-shader and its inverted-hull outline, in
//!   one pipeline per static permutation (see "The anime character shader" below), and the
//!   environment uber-shader, per coverage (see "The environment shader" below), the foliage
//!   shader, per coverage ([`foliage`]), and the fog card shader. A mesh's submeshes may draw with
//!   any of the four families, and draws group by pipeline (see "Material families on meshes"
//!   below).
//! - Fog ([`fog`], M1 W3): the packet's distance and height [`Fog`], applied by one shared
//!   function at the final color of every world pass (the environment, anime characters and
//!   their outlines, fog cards) and of the sky, and fog cards: translucent sheets of the fog
//!   card family on meshes, lit by the fog, faded near the lens and (on the desktop tiers) where
//!   they meet geometry, drawn last and back to front.
//! - Foliage ([`foliage`], M1 W6): trees, bushes and grass of the foliage family, lit by the
//!   environment's light model on the volume normals the cooker transferred onto their cards,
//!   with canopy occlusion, a color gradient and light through thin leaves; swayed in the vertex
//!   stage by the packet's [`Wind`] in three levels (trunk, branch, leaf), alike in every pass
//!   that draws them, with the wind in their velocity; cut out by an alpha test in the main pass,
//!   the scene prepass and the shadow casters, or by alpha-to-coverage under MSAA where the tier
//!   asks ([`FoliageSettings`]).
//! - The sky (`sky`, M1 W2): the gradient, stars and the Milky Way, the sun's and moon's discs and
//!   glows, cirrus and the card clouds' band, drawn at the far plane in the main pass from the
//!   packet's [`SkyLayers`] and its sky material, and a sky-only cubemap updated a face a frame
//!   for the probes and reflections that will read it (see "The sky" below).
//! - Local lights (`lights`, M1 R5): the packet's plain-data [`LocalLight`]s, culled to the
//!   view and capped, then listed for the mesh shaders by clustered forward+ (a 16 × 9 × 24 grid
//!   binned by a compute pass, desktop) or per-object lists (at most 4 per instance, chosen on
//!   the CPU, mobile), as [`RenderSettings::light_path`] says. Both paths run on every backend.
//!   The anime character shader lights by each local light in the key light's two-tone model,
//!   the environment shader through its terminator and wrapped falloff.
//! - Characters' light rigs and faces on the GPU (`characters`, M1 C2 and C3): each frame's
//!   [`CharacterInstance`]s, one entry per character in a per-frame buffer that the anime
//!   character shader finds through the instance (see "The anime character shader" and "Faces"
//!   below).
//! - Shadows ([`shadows`], M1 R4): the world key light's cascaded shadow maps, stable and partly
//!   cached, each character's own shadow (a map on desktop, its body's capsule on phones), the
//!   receivers every surface shader calls, and the pass that brings the environment's shadow to
//!   faces whole (see "Shadows" below).
//! - Post-processing ([`post`], M1 R3): HDR scene color, bloom, the tone curve and grade baked
//!   into blended 32³ LUTs, and the fused composite into the target, driven by plain-data
//!   [`PostParams`]; and in the same fused pass (M1 P2) anime compositing's layers: the
//!   diffusion glow, characters' own bloom (a mask in scene color's alpha), gradient ("para")
//!   layers, lens flares from the sun, a tinted vignette and the player's sharpening.
//! - Compute skinning and morph targets (`skinning`, M1 R2): every instance of a skinned or
//!   morphed mesh draws from output streams of its own, which one dispatch a frame writes from
//!   the packet's poses ([`SkinnedInstance`]); see "Skinned and morphed meshes" below.
//! - The prepasses (M1 R2, C5): a character prepass of the listed characters' depth and IDs on
//!   every tier, into persistent targets the main pass reads, and a scene prepass of depth,
//!   world normals and velocity as frame-graph transients, which the graph culls while nothing
//!   reads them ([`PrepassTargets`]).
//! - Temporal AA and upsampling ([`temporal`], M1 R7): the projection's sub-pixel jitter, the
//!   responsive mask, and a resolve that accumulates frames in a history at the target's size,
//!   upsampling a smaller scene, before bloom and grading. The desktop tiers resolve with it;
//!   phones keep 4× MSAA in tile memory ([`RenderSettings::temporal`]).
//! - Shader sources (`shaders`, M1 A8): the WGSL files compiled in ([`ShaderFile`]), the
//!   programs built from them, and rebuilding pipelines from edited text
//!   ([`Renderer::set_shader_sources`]: hot reload and live link), checked with naga first so an
//!   error names its file and line, and kept out if anything fails. The renderer never reads a
//!   file (rule R5).
//! - Replacing uploaded objects in place, under the same handles (M1 A8): materials
//!   ([`Renderer::update_material`], which also rebinds replaced textures) and meshes
//!   ([`Renderer::update_mesh`], whose old room in the mesh pool is given back).
//! - The frame graph ([`graph`]): pass culling and ordering, pooled transient textures,
//!   parallel recording and the frame's one submit.
//! - Texture uploads ([`texture`], M1 A2): cooked KTX2 textures to sampled GPU textures with
//!   every mip level, after checking that the device supports their format.
//! - Quality tiers ([`quality`], M1 R6): the tier table (`tiers.ron`, a versioned RON document
//!   compiled in), selecting a tier from the device class, [`GpuCaps`](sapphire_rhi::GpuCaps)
//!   and RAM, the `r.*` overrides, and each frame's render scale with dynamic resolution. The
//!   scene renders at the render scale ([`Renderer::set_render_scale`]) and post's final pass
//!   upsamples it to the target (see "Render scale" below).
//! - Debug views ([`debug_view`], M1 F5): the frame drawn as a wireframe, as overdraw or as the
//!   local lights each pixel walks, picked per frame ([`Renderer::set_debug_view`], the
//!   `r.debug_view` cvar). Each replays the main pass through copies of its own pipelines into
//!   a depth and stencil it reads, and draws over the frame after post. Off, the default, builds
//!   and draws nothing.
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
//! - Outline widths (`AnimeCharacterParams::edges.z`) are pixels at a 1080-pixel-high target,
//!   at the width curve's mid distance through a 45° lens (see "Outlines").
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
//! | 2 | shading: octahedral normal, and the outline normal in the normal's tangent frame (M1 C4) | 8 | surface shading, outline, scene prepass |
//! | 3 | surface: unorm16 UV, vertex color (AO, outline width, outline Z offset) and flags as unorm8 × 4 | 8 | surface shading, outline and its prepass (width and Z offset) |
//! | 4 | previous positions: where the vertex was when last drawn, in slot 1's format | 8 or 12 | scene prepass (velocity, surfaces and hulls) |
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
//! - **Character prepass** ([`CHARACTER_PREPASS`], every tier, M1 C5; the `character_prepass`
//!   module): the depth (`Depth32Float`) and IDs (`Rg16Uint`: the character's entry and its
//!   part, body, face, hair or eye; R the nearest surface's, G the nearest but for hair, which
//!   hair draws last to leave alone, the **eye depth mask** of M1 C6) of the opaque and
//!   alpha-tested anime character surfaces of the instances the frame lists as characters
//!   ([`FramePacket::characters`]), static or deformed: opaque ones position-only, alpha-tested
//!   ones (and fading characters', M1 C7) through a masked pipeline that cuts them out as the
//!   main pass does (see "Material layers and the proximity fade"). Its targets are the
//!   renderer's own, persistent and imported into the graph, so it runs every frame with a view,
//!   and group 0 binds them (bindings 8–10) for the main pass: the rim, the bangs' shadow on
//!   faces, eyes through hair and the eyes' layers read it (see "Rim, hair and eyes through
//!   hair"), and temporal AA's responsive mask, which gives the characters it holds their
//!   reactivity (P1's 0.5; [`temporal`]).
//! - **Scene prepass** ([`SCENE_PREPASS`]): depth, world normals (octahedral, `Rg16Float`) and velocity
//!   (`Rg16Float`, texture coordinates) of every opaque instance, then of their outline hulls
//!   (M1 C4), which also write the outline mask (`R8Unorm`, 1 where a hull is nearest). The anime
//!   character family's alpha-tested surfaces and hulls are cut out of it as of the character
//!   prepass (M1 C7), foliage's cards by foliage's own pipelines (M1 W6).
//!   Characters move by their skinned positions, static meshes by their instance transform, and
//!   both by the camera's, never by temporal AA's jitter; a hull moves with its surface's
//!   vertices, so its depth is the main pass's hull's, and its velocity is its line's, the
//!   silhouette's motion (M1 P1). TAA (R7), GTAO and screen-space edges read it.
//!
//! Temporal AA reads both (its depth, velocity and responsive mask). Without it nothing reads
//! the scene prepass yet, so the frame graph culls it: a boxed closure a frame and no GPU time.
//!
//! # Render scale
//!
//! Every pass before post's final one renders at the scene's size, [`scaled_size`] of the target
//! at the render scale: the main pass's attachments and the prepass targets, the views (whose
//! viewport sizes outline widths and light-cluster tiles) and the bloom chain. Post's fused pass
//! writes the target and upsamples bilinearly where the scene is smaller; at scale 1 it loads
//! texel for texel, so frames are exactly what they were without a render scale. Under temporal
//! AA the resolve writes the target's size instead, upsampling from jittered frames (TAAU), and
//! bloom and the fused pass follow it at that size. Passes added after the renderer's (the debug
//! overlay, UI) draw at the target's size. Each new size is one new set of transients in the
//! frame graph's pool, which is why dynamic resolution moves in steps.
//!
//! # Binding budget
//!
//! | Group | Binding | Stages | Contents | Written |
//! |---|---|---|---|---|
//! | 0 frame | 0: uniform | vertex, fragment | sun, sky, time, light counts, cluster depth slices, day/night, ambient, shadow tint, weather, the outline cutoffs' scale | `prepare` |
//! | 0 frame | 1: storage | fragment | local lights (`LocalLight`, 64 bytes each) | `prepare`, when they changed |
//! | 0 frame | 2: storage | fragment | light lists: one per cluster, or one per instance slot | the cluster pass, or `prepare` for changed instance lists |
//! | 0 frame | 3: storage | fragment | per-character data (`CharacterData`): the world's key light, then the frame's characters' light rigs, faces, shadows, ambient and fades (M1 C7), 240 bytes each | the uploads pass, when they changed; the face pass writes each entry's shadow, and with a probe grid the ambient pass its ambient |
//! | 0 frame | 4: uniform | fragment | shadows (`ShadowUniforms`): light space, every cascade's and character map's area and depth transform, the capsules | the uploads pass, while shadows are on |
//! | 0 frame | 5, 6: depth textures | fragment | the shadow cascades (one array) and the character atlas | the shadow passes |
//! | 0 frame | 7: comparison sampler | fragment | shadow lookups, and the rim's depth step | — |
//! | 0 frame | 8, 9: textures | fragment | the character prepass's depth and IDs (`texture_depth_2d`, `texture_2d<u32>`; M1 C5) | the character prepass |
//! | 0 frame | 10: sampler | fragment | nearest, clamped: the character prepass's gathers (non-filtering) | — |
//! | 0 frame | 11: uniform | vertex, fragment | ambient (`AmbientUniforms`): the probe grid's placement and atlas layout, GTAO's texture size, the capsules, the sky faces the projection reads | the uploads pass |
//! | 0 frame | 12: 3D texture | vertex, fragment | the probe grid's relit atlas (one black texel without a grid) | the ambient pass's relight |
//! | 0 frame | 13: 3D `u32` texture | vertex, fragment | the probe grid's brick table | grid upload |
//! | 0 frame | 14: cube texture | fragment | the reflection cube's roughness mips (the sky cube without a grid) | the ambient pass's prefilter |
//! | 0 frame | 15: texture | fragment | GTAO's visibility and depth at half resolution (one open texel without GTAO) | the ambient pass's GTAO |
//! | 0 frame | 16: sampler | vertex, fragment | linear clamped: the atlas and the reflection cube | — |
//! | 1 view | uniform, dynamic offset | vertex, fragment, compute | matrices (this frame's and the last drawn one's view-projection), camera, viewport | `record` |
//! | 2 scene | 0: storage | vertex | instance transforms (current and last drawn) and character entries, by slot | `prepare`, changed instances only, and once more when they come to rest |
//! | 2 scene | 1: storage | vertex, fragment | the drawn family's material blocks, by slot: a scene group per family meshes draw with, each its own layout | the uploads pass, changed slots only |
//! | 3 material | 0–7: textures | fragment | one material's texture slots (the anime character family's eight, the environment's five), white 1×1 where unbound | material upload or update |
//! | 3 material | 8–10: samplers | fragment | trilinear repeating; linear clamped for ramps; nearest for material IDs | — |
//!
//! Vertex buffers: slot 0 is the draw records, slots 1–4 a mesh's streams (above). The
//! prepasses bind groups 0–2 like the main pass, with the anime character family's scene group
//! for every family's draws, and read no storage buffer beyond that group's: the instances, and
//! the anime character blocks for the outline hulls' widths (M1 C4).
//!
//! The view group's buffer holds every view at a stride of the device's uniform offset
//! alignment: the main view (offset 0, written every frame), then the sky cube's six faces
//! (written once), then the shadows' views, every cascade and then every character map (written
//! in the frames that draw them). The sky's pipelines bind groups 0 and 1 as the scene's, their own group 2 (the
//! frame's `SkyUniforms`, one uniform at binding 2, past the scene group's bindings
//! `frame.wgsl` declares) and the sky family's group 3 (five texture slots and the trilinear
//! sampler), so the sky reads no storage buffer beyond group 0's.
//!
//! A debug view ([`debug_view`], M1 F5) binds groups 0–3 as the main pass does in its replay,
//! with group 1 its own buffer holding the main view without TAA's jitter. Its pass over the
//! frame binds group 0, that view group, and a group 2 of its own: the replay's stencil
//! (`texture_2d<u32>`, binding 0) and depth (`texture_depth_2d`, binding 1), so it reads one
//! storage buffer, the light lists. The wireframe's line indices pass (`cs_debug_lines`) binds a
//! group 0 of its own, all compute: the pool's indices copied (binding 20, read-only) and the
//! line indices (21, read-write), past the frame group's bindings that its file declares too.
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
//! and a sampler, 1 the textures the pass reads (at most five: the fused pass's input, bloom,
//! the two LUTs and the diffusion glow's level; its input, the constants and the sampler are
//! visible to its vertex shader too, which measures the lens flare's visibility, M1 P2) and no
//! storage buffer. Temporal AA's two passes do the same
//! (`shaders/temporal.wgsl`, two samplers): the resolve, a compute pass, reads six textures,
//! writes the history as a storage texture and binds no storage buffer. The view
//! group's uniforms carry the jitter (appended in M1 R7: the unjittered `clip_from_world` and
//! the jitter itself). The light-cluster pass (`shaders/clusters.wgsl`)
//! binds its own group 0 (the frame constants, the lights read-only and the cluster lists
//! read-write: the same buffers; and at binding 3 its own buffer of the lights in view space,
//! which its first dispatch writes) and the view group as group 1. The ambient pass's dispatches
//! (`shaders/probes.wgsl`, `occlusion.wgsl`, M1 W5) bind an "ambient frame" group 0 of their
//! own (the frame constants and bindings 11–13 and 16, without the characters, which the
//! character dispatch binds read-write as its group 1) and their own group 3, or the view group
//! and group 2 for GTAO: at most two storage buffers a dispatch (the relight's grid words and sky
//! light).
//!
//! Storage buffers per stage are budgeted against the 4-per-stage floor of Vulkan 1.1 phones
//! (see `sapphire_rhi`): vertex 2 (instances, materials), fragment 4 (materials, lights, light
//! lists, characters), compute 3 in the cluster pass (lights, cluster lists, view-space lights)
//! and 3 in the skinning pass (sources, frame data, outputs). That leaves none in the fragment
//! stage, two in the vertex stage and one in each compute pass. The fragment stage's last went
//! to per-character data (M1 C2) because every later per-character need joins it rather than
//! taking a binding: faces' head basis and expressions (C3, beside the rig), the environment's
//! shadow over the face (R4), outlines lit by the rig (C4). Frame-wide data that is not per character goes into
//! uniforms or textures: the shadows' uniforms, cascades and character atlas (R4), the character
//! prepass's targets (C5), the ambient uniforms, the probe grid's atlas and brick table, the
//! reflection cube and GTAO (W5), whose relight packs the grid's data into one storage buffer
//! under the same floor. Group 3 is the last of the 4 bind groups the floor allows; the
//! prepasses, which read no material textures, leave it out, but for the masked ones (M1 C7) and
//! foliage's, which read the base color map for the alpha test. The anime character family binds 8
//! of the 16 sampled textures and 3 of the 16 samplers a fragment stage gets (trilinear, ramp,
//! and nearest for material IDs), leaving 8 textures for frame-wide ones: shadows take two (the
//! cascades and the character atlas) and a comparison sampler (R4), the character prepass two
//! (its depth and IDs) and a nearest sampler (C5), and W5's ambient the last four (the atlas, the
//! brick table, the reflection cube, which C7's reflection add reads too, and GTAO) and a linear
//! sampler: every sampled texture a fragment stage gets, and 10 samplers left; the other families
//! bind 1 to 5 (the environment's
//! pipelines read five textures and two samplers, and the same four storage buffers as the anime
//! character's, its own family's in place of that one's). Local lights take
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
//!   them, alpha test, transparency, faces, hair and eyes are shader variants today, through
//!   pipeline-overridable constants, and each
//!   variant's pipelines are built when a mesh or a material edit first needs them. Everything
//!   else (ramp or bands, a deep band, which maps are bound) is a branch on the block that every
//!   pixel of a draw takes alike.
//! - **Draw order** within the main pass: opaque surfaces, alpha-tested ones, their outlines
//!   (only for materials with one), eyes and brows through hair (M1 C5), the sky, then
//!   transparent surfaces blended over it. Draws are grouped by permutation; transparent ones
//!   keep the draw list's order.
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
//! - **Cast shadows** enter through one function, `cast_shadow`. Its contract with the light rig:
//!   it returns how much of the world's key light (the sun or the moon) reaches the point past
//!   environment casters alone, never the character's own geometry (characters don't
//!   self-shadow), however the rig turns the character's key; the split takes it as it takes
//!   vertex AO. R4 fills it with `cascade_visibility()` (`frame.wgsl`), since the cascades hold
//!   environment casters alone; an instance drawn with this family that isn't a character (a
//!   prop, the sandbox's pedestal) takes `shadow_visibility()`, as environment surfaces do. The
//!   face permutation never calls it (see "Faces").
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
//! - **The bangs' shadow** (M1 C5): the character's own hair, toward the key light across the
//!   screen in the character prepass, occludes the split as a cast shadow would, so the face
//!   darkens into its authored shadow tone (see "Rim, hair and eyes through hair").
//! - **No cast shadow:** shadow maps are too coarse and unstable at face scale, so the face
//!   permutation doesn't call `cast_shadow` and reads no shadow map. An environment shadow
//!   reaches a face whole, per character (R4): the face pass samples the cascades over a sphere
//!   around the head once a frame and writes the result into the character's entry
//!   (`CharacterData::shadow`), which the face takes as occlusion of its split, with its
//!   always-lit eyes and brows; never shadow-map texels on it.
//!
//! # Rim, hair and eyes through hair (M1 C5)
//!
//! The character prepass's readers, in `shaders/anime_character.wgsl` (its header has the
//! formulas) on every tier:
//!
//! - **The depth-offset rim** replaces C1's key-light Fresnel rim on characters' bodies, faces and
//!   hair. A pixel looks across the screen along its normal, the material's width in pixels at
//!   1080 rows (× the region's width scale, and like an outline's × its distance curve and model
//!   scale, so rim and lines keep their proportions at any distance and lens, though not its LOD
//!   cutoff), and compares the
//!   character depth there against its own plus the material's threshold (a filtered
//!   comparison): the rim is where the character ends against what lies well behind it, or
//!   against no character. Where the pixel there is the same character's, the region's overlap
//!   scales it (an arm over the torso). A Fresnel gate and the region's strength scale it. Per
//!   light, the light gate keeps it to the lit side by the material's amount (WuWa) and ZZZ's
//!   back-light weighting scales it by 1 − V·L; it takes the light's radiance, the rig's rim
//!   scale and a tint (half albedo, half white, × the region's rim color). **Local lights rim
//!   too,** through the same mask: a rim from only the strongest light would pop as lights
//!   reorder. Unlisted instances, eyes and transparent surfaces have none.
//! - **Hair** (the hair permutation): the hair mask's occlusion (B) and highlight (R, with G
//!   shifting it), and a band in the key light's color over one of three placements, picked on a
//!   branch: a painted band where N·H on the GI normals passes 1 − width, an angel ring at a
//!   height of the view-space normal (it holds still on screen), or Kajiya-Kay along the
//!   strands. **Decision: strands run along UV v** (root to tip), the direction taken from the
//!   screen derivatives of the position and v, since meshes carry no tangents; hair cards and
//!   strips are authored that way.
//! - **The bangs' shadow on faces:** the face pixel steps toward the key light across the screen
//!   (the material's distance at its depth, the light's view-space direction unnormalized, so a
//!   light along the view casts a short shadow) and gathers the prepass there: the bilinearly
//!   weighted share of its own character's hair more than 5 mm nearer than the face, × the
//!   strength, occludes the split. **Decision: the same on mobile,** where the plan had offset
//!   bangs and a stencil: the prepass runs on every tier anyway, and this costs two gathers and
//!   no extra draw.
//! - **Eyes and brows through hair:** face materials (their always-lit mask) and eye materials
//!   that ask for it (`MaterialFlags::THROUGH_HAIR`) draw a second time after the outlines,
//!   through a pipeline of their variant's own (`fs_through_hair`, blended, no depth write):
//!   where the prepass's ID is the character's own hair and the eye lies within the material's
//!   distance behind it, at the material's opacity faded as the camera turns from head-forward
//!   past its yaw and pitch, at the hair's depth nudged nearer, so what hides the hair hides
//!   them, and fogged as the surface is (`fogged` at the eye's own position with its character's
//!   share, M1 W3). The rim and the hair's highlight are lit before the surface's one `fogged`.
//!   Writing depth turns early depth testing off, so the pass tests the prepass first and shades
//!   only the 2 × 2 quads with a pixel under the character's own hair (an answer made the same
//!   across each quad, so the shading's derivatives hold).
//!   **Decision: the ID's two part bits do the stencil's work:** a hardware stencil would
//!   need the main pass's depth-stencil format and a third pass on tilers, while the IDs exist
//!   anyway. They are also C6's hook: eye and brow pixels are marked, and eyes have their own
//!   permutation and through-hair pipeline.
//! - **Counters** ([`CharacterPrepassStats`] in [`RenderStats::character_prepass`]): the
//!   prepass's draws, the characters with a face under the bangs' shadow, the eye and face draws
//!   through hair, and the eye layers' draws (M1 C6); [`RenderStats`] counts hair and eye draws by
//!   variant too.
//!
//! # Anime eyes (M1 C6)
//!
//! Eyes are layered meshes on the eye permutation, built on C5's eye part and its hook, not a
//! system beside them. `shaders/anime_character.wgsl`'s header has the formulas.
//!
//! - **Layers:** each eye material names one (`EyeShading::layer`, the block's `modes.w` third
//!   byte): the eye itself, opaque or alpha-tested, which writes depth and the prepass's eye part;
//!   then, blended, the upper lid's shadow on the eye white (in the eye's shadow tone), the iris
//!   and the highlights. The blended layers are one variant (`SurfaceVariant::EYE_LAYERS`, eye ×
//!   transparent: no new permutation bit), drawn with the transparents after the sky, sorted by
//!   layer and then draw (the draw list's sort key, from the material record's layer), with depth
//!   compared greater-or-equal so a layer modeled on the eye's own surface passes.
//! - **The eye depth mask** is the prepass's second ID. Hair draws last, through a prepass
//!   pipeline that writes R alone, so G holds the nearest surface but for hair. A layer blends only
//!   where G is its character's own eye: a lid (the face) closing over the eye hides the iris and
//!   highlights whatever their depth, and a gaze carrying the iris past the eye white is clipped
//!   there. Eyes through hair test G against their own part too, so a closed eye stays hidden
//!   under the bangs, and the layers draw through by their coverage.
//!   **Decision: the mask is a second channel, not a stencil or another pass.** The prepass
//!   already draws every eye; the second channel costs 2 bytes a pixel and one more write mask.
//! - **Parallax refraction fake** on the eye and iris layers: the base color is read offset along
//!   the surface by the view's along-surface part × the material's depth × the eye mask's R (or
//!   a dome over the iris ellipse, or whole), through the gradients of u and v from screen
//!   derivatives (no tangents). Offset-limited, so it never runs off at grazing angles.
//! - **Highlights** drift (a 2 : 3 Lissajous across the model's front, in the vertex stage) and
//!   sparkle (a sharpened sine), on the packet's time (R4), at a phase per instance slot.
//! - **Fog:** every layer ends in `fogged` with its character's share, as the surface does.
//!
//! # Material layers and the proximity fade (M1 C7)
//!
//! `shaders/anime_character.wgsl` (its header has the formulas; spec §5.12 "Anime character
//! shading" and "Proximity fade"):
//!
//! - **Layers** over the base shading, each off until a region turns it on, so materials from
//!   before them draw exactly as they did (but for a matcap they named, which now shades), and
//!   each a branch on the block's flags
//!   (`AnimeCharacterParams::FLAG_SPECULAR` and on, `modes.x` bits 12–16), compiled in only for
//!   materials with a layer (`MaterialFlags::LAYERS`, the permutation bit that took the skinned
//!   bit's place) and for eyes: the stylized highlight (thresholded Blinn or minimal GGX of N·H on the key light,
//!   sized by the control map's B and masked by its R, on the lit side, tinted by the albedo);
//!   the matcap (one color or four grayscale spheremaps by the view-space normal, added in the
//!   key light or multiplying the albedo, at a lit and a shadow strength); the reflection add
//!   (above metallic 0.9: W5's reflection cube along the reflected view, the sky at the
//!   ambient's scale, at the region's roughness's mip, × the albedo, on every tier, which the
//!   ambient pass prefilters while a reflective material draws, grid or not); stockings (denier
//!   by N·V, a sheen along the fabric's UV axis and a weave faded to its average before it
//!   aliases); and emission, unlit, breathing over a period at a phase per
//!   instance slot. A body's layer mask (the surface mask slot: R matcap, G metallic, B
//!   emission, A stocking density) paints where each shows. An emissive pixel's glow share ×
//!   the material's bloom is written as the world's share of the pixel (scene color alpha), so
//!   post blooms it as the world's light while the character around it keeps its restrained
//!   bloom (P2's character mask). Compiled into every variant, the never-taken branches cost
//!   the main pass a few percent; behind the bit, materials without a layer pay nothing for
//!   them (spec §5.12 has the measurements).
//! - **The proximity fade** ([`CharacterInstance::opacity`], which `sapphire_world::fade`
//!   resolves from the camera's distance to the body): a screen-space 4 × 4 ordered dither,
//!   turned by the character's entry and, under temporal AA, through its sixteen thresholds in a
//!   16-frame cycle, that every pass drawing the character shares (the `characters` module's docs have the passes): its opaque draws move to
//!   the alpha-tested variants while it fades, so opaque pipelines never discard; the character
//!   and scene prepasses and the outline hulls' prepass draw those variants through masked
//!   pipelines (`prepass.wgsl`, `outline.wgsl`'s `*_masked`), which bind the family's group 3 and
//!   cut out by the alpha test (the base color's alpha × its map's against the cutoff) and the
//!   dither as the main pass does, so the rim, the bangs' shadow, eyes through hair, the
//!   responsive mask and reprojection see the character where it is drawn and alpha-tested
//!   cards at their drawn shape. Under temporal AA the turning pattern resolves into a blend:
//!   the masked scene prepasses mark a fading character's surfaces and hulls in the outline
//!   mask, and the responsive mask gives them the world's reactivity, so the history keeps every
//!   turn of the pattern, moving or not, and the dither's order puts the character and what lies
//!   behind it in every 3 × 3 neighborhood from a fade of 3/16 to 13/16, so the resolve's clip
//!   keeps the blend;
//!   under MSAA the alpha-tested variants' covered entry points (`fs_*_covered`) write a share
//!   of each pixel's samples instead, and the single-sampled prepasses keep the pixels where it
//!   isn't empty. A fading character takes no rim; its shadow stays whole. Only the anime
//!   character family fades.
//! - **Cutting pixels out** (the alpha test, the dither): decided after every derivative,
//!   discarded at the entry points' one exit after the shading, with only quads cut out whole
//!   leaving early (frame.wgsl's `quad_any`; docs/conventions.md): Apple's Metal compiler has
//!   miscompiled earlier per-pixel discards, writing the cut pixels black with their depth.
//! - **Counters:** [`RenderStats::faded_characters`] and [`RenderStats::faded_draws`].
//!
//! # The environment shader (M1 W4)
//!
//! `shaders/environment.wgsl` is one uber-shader for every environment material: a stylized GGX
//! for the world, softer and less contrasty than the characters drawn over it, so they read on top
//! (look pillar 4). The file's header has the model, and the `environment` module its tiers:
//!
//! - **The key light** is the world's ([`FramePacket::sun`], the frame uniforms' `sun_*`), never a
//!   character's rig. Its N·L, pushed toward shadow by the cast shadow in half-Lambert space,
//!   passes a terminator remap `smoothstep(-w, w, ·)` whose half-width grows with view distance,
//!   so far shading softens into painted masses; the lit side falls off as wrapped Lambert; inside
//!   the terminator the key light keeps time of day's shadow tint ([`FramePacket::shadow_tint`])
//!   times the material's, so shadows are colored, never black; a terminator band (a one-row ramp
//!   read at the terminator's value) tints the transition.
//! - **Ambient light** is the frame's hemisphere (`ambient_light`, shared with the characters'
//!   GI), occluded by vertex AO and the mask map's AO and cavity.
//! - **Specular** is GGX under a clamp, with the roughness remapped into the material's range,
//!   plus the hemisphere's reflection; a zero specular mask leaves a surface matte at every angle.
//!   The tier picks the model ([`EnvironmentSpecular`]): full on desktop, approximate on
//!   Mobile-High, none on Mobile-Low.
//! - **Distance desaturation** greys the albedo with view distance, keeping the light's hue;
//!   **emission** adds HDR light.
//! - **Local lights** come from the same lists the anime character shader walks.
//! - **Cast shadows** enter through one function, `cast_shadow`, which R4 fills with
//!   `shadow_visibility()` (`frame.wgsl`): the key light's visibility past the cascades (the
//!   environment's casters) and every character's own shadow (its map on desktop, its capsule on
//!   phones). It joins the terminator's input, so shadows take its remap, tint and band.
//! - **Wetness:** the frame carries the weather's wetness and puddles ([`Weather`]) to the GPU for
//!   the wet surface model that comes with weather; no shader reads them yet.
//! - **Outlines:** the environment draws no inverted hull in M1 (decision): its edges are the
//!   spec's screen-space edge pass, a later desktop option; props that should read like
//!   characters use the anime character family.
//! - **The final color** is produced in one place at the end of the fragment shader, where the
//!   frame's fog joins it (`fogged`, M1 W3).
//!
//! # Shadows (M1 R4)
//!
//! The [`shadows`] module has the design; in short:
//!
//! - **Who casts and receives:** the frame's character list classifies. Environment instances
//!   cast into the cascades and receive everything; characters cast only their own shadow (a
//!   tile of the character atlas on desktop, their [`CharacterBody`]'s capsule on phones) and
//!   receive the cascades alone, so they never shadow themselves.
//! - **The cascades:** 4 on desktop and 2 on phones ([`ShadowSettings`] per tier), fit to
//!   spheres around the view's slices and snapped to texels under a quantized key light, the far
//!   ones cached and drawn in turns; a tent PCF that receivers' splits and terminators threshold.
//! - **Passes:** side effects before the main pass (each cascade drawn, the character atlas, the
//!   face pass), never jittered; [`RenderSettings::shadows`] `None` draws none and compiles the
//!   shaders as they were. [`ShadowStats`] in [`RenderStats::shadows`] count them.
//! - **Mesh bounds** live in the mesh records (the cooked bounds as a sphere), which the
//!   per-object light lists, the outlines' LOD cutoff and the casters' culling read.
//! - **Outlines:** lit lines receive the cast shadow as their surface does (a face's line the
//!   face pass's whole value); hulls cast none.
//!
//! # Material families on meshes (M1 W4)
//!
//! A mesh's material slots bind anime character, environment, fog card (M1 W3) or foliage (M1 W6)
//! materials, mixed freely, and [`Renderer::upload_mesh`] takes all four (the sky family is
//! [`RenderError::NotDrawable`]; foliage needs a static mesh with a foliage stream, else
//! [`RenderError::NoFoliageStream`] or [`RenderError::DeformedFoliage`]). Each submesh draws
//! through its family's shader:
//!
//! - **Surface variants** are a coverage (opaque, alpha-tested, transparent) and a shading: the
//!   anime character shader for a part (body, face, hair, eye), the environment shader, the
//!   foliage shader (M1 W6), or the fog card shader (always transparent). Nineteen in all, each a
//!   pipeline per position format, built when a draw first needs it; the opaque and alpha-tested
//!   face and eye variants have a through-hair pipeline too (M1 C5).
//! - **Draws sort by pipeline:** the draw list groups them by position format and variant, so the
//!   families never interleave whatever order instances and submeshes come in. In each coverage
//!   characters draw before the world (the environment, then foliage), so depth rejects the
//!   world behind them; then the characters' outlines, the sky, transparent surfaces of both,
//!   and fog cards last, back to front by their instances' bounds (the [`fog`] module).
//! - **Scene groups:** each family's pipelines bind group 2 with its own parameter buffer beside
//!   the instances (and a third binding of the family's own: fog cards' depth, foliage's wind);
//!   the main pass rebinds group 2 only when the family changes.
//! - **The prepasses** draw every family's opaque and alpha-tested surfaces into the scene
//!   prepass (depth, normals, velocity, which temporal AA reads), foliage through pipelines of
//!   its own that bind group 3 and cut its cards out, and the anime character family's
//!   alpha-tested variants through masked pipelines that do the same (M1 C7); the character
//!   prepass draws the anime character family's surfaces of the instances the frame lists as
//!   characters alone, so a skinned prop or an anime-shaded pedestal is never taken for a
//!   character.
//!
//! # Outlines (M1 C4)
//!
//! Outlines v2 (`shaders/outline.wgsl`, whose header gives the formulas; spec §5.12
//! "Outlines") draw the anime line art as inverted hulls, one per outlined draw, after the
//! opaque surfaces in the main pass and again in the scene prepass:
//!
//! - **Outline normals** are smoothed normals the cooker bakes or the artist authors, stored in
//!   the tangent frame of the vertex's own normal, a frame derived from the normal alone
//!   (`sapphire_assets::mesh::outline_frame`), so split vertices at a hard edge share one hull
//!   direction and the hull stays closed. Compute skinning carries them across with the normal
//!   (see the `skinning` module).
//! - **Width:** the material's width in pixels at 1080 rows × the region's scale × vertex color
//!   G × the material's model scale × a two-segment curve over the *effective distance*, the
//!   view depth normalized to a 45° field of view, so a line keeps its width wherever a
//!   character looks the same size. Outside the curve the line keeps its width in the world.
//! - **Extrusion** in view space, with the normal's depth replaced by the material's flatten
//!   amount, and a push-back along the view ray (Z offset × vertex color B), both through one
//!   unjittered matrix so the prepass's hull lands where the main pass's does.
//! - **Color:** the region's outline color, blended toward it × albedo, lit as the surface is:
//!   the rig's key light through the light split and shadow tone, plus its GI flattened toward
//!   the camera, by the material's lighting weight.
//! - **LOD cutoff:** past a material's cutoff (effective meters, × `r.outline_distance`,
//!   [`Renderer::set_outline_distance`]) the frame drops the hull, per draw, on the instance's
//!   bounding sphere (the `outlines` module); the width fades out over the cutoff's last
//!   quarter first. [`RenderStats::outline_draws`] and [`RenderStats::outlines_culled`] count
//!   both.
//! - **Velocity:** the scene prepass draws each hull with this frame's and the last drawn
//!   frame's clip positions and writes its line's velocity, the silhouette's motion rather than
//!   the hull point's (M1 P1), and the outline mask, which makes lines fully reactive in the
//!   responsive mask ([`temporal`]): under TAA a moving line follows the silhouette and is
//!   drawn from the current frame.
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
//!   branch every pixel takes alike, and leaves the gradient exactly as it was; under the
//!   horizon, and where a cloud texel covers nothing, pixels skip the layers that add nothing.
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
//! (`shaders/materials.wgsl`, mirroring `sapphire_assets::material`): 1,584 bytes for the anime
//! character family (1,120 until the material layers, M1 C7; 944 until the depth-offset rim's
//! per-region settings, M1 C5; 896 until outlines v2, M1 C4), 80 to 160 for the others. One shared array would pad every slot to the
//! largest block, and a pipeline only reads its own family, so each family's pipelines bind
//! their buffer as group 2 binding 1 and the storage-buffer budget above is unchanged. The sky
//! draws one material at a time and reads its block from the CPU mirror into its frame uniforms
//! (see "The sky"), so the GPU copy of the sky family's buffer goes unread. A draw
//! record's `material` is the slot in that buffer; a slot is kept for the renderer's lifetime
//! and written through a CPU mirror, so editing a material uploads its one block with the next
//! frame and nothing else. Every family uploads and binds its textures today; meshes draw with
//! the anime character and environment families, so [`Renderer::upload_mesh`] accepts their
//! materials, and the sky draws with a sky material.
//!
//! Material textures are sampled with explicit gradients (taken once per pixel, in uniform
//! control flow) so that unbound slots can be skipped on a branch, and every sample goes through
//! the slot's swizzle code from the block, which makes ASTC's two-channel textures read as
//! BC5's do.
//!
//! # Later milestones
//!
//! - **M1:** mesh residency (the pool's buffers already hand out ranges first fit and take them
//!   back when a mesh is replaced) with generational [`MeshHandle`]s; mesh
//!   uploads through a persistent staging ring in place of `Queue::write_buffer`;
//!   material/mesh draw sorting within each pipeline's group (draws already group by pipeline,
//!   M1 W4). The [`FramePacket`] grows with the systems that
//!   feed it: VFX, UI and debug draws, plus UI-inset views (shadow views are the renderer's own,
//!   derived from the key light).
//! - **Skinning** (see the `skinning` module): skinning only visible instances, at their
//!   selected LOD's prefix, with posed bounds from their joints; exact normals for
//!   non-uniformly scaled joints; outline-normal deltas for morph targets that bend hard edges;
//!   vertex pulling for multi-draw-indirect of skinned instances.
//! - **Prepasses:** their other consumers (GTAO); and, now that TAA turns MSAA off on desktop,
//!   letting the main pass reuse the scene prepass's depth (load it and test `Equal`, skipping
//!   overdraw), which needs the prepass to cut the environment's alpha-tested surfaces out too
//!   (the anime character family's are, M1 C7, and foliage's, M1 W6).
//! - **Local lights** (see the `lights` module): cluster occupancy counted on the GPU, light
//!   layers (VFX lights kept off faces and rims), local-light shadows, and two-level culling for
//!   hundreds of lights.
//! - **LOD selection**, with culling: the pool holds every LOD's indices and ranges, and each
//!   cooked LOD carries its error in object-space meters, to project into pixels per view. Until
//!   then draws use LOD0.
//! - **Android pre-rotation:** whether the compositor rotates every frame has to be measured
//!   on a real phone first (spec open question Q14).
//! - **Upscaling** (spec §5.12, "Anti-aliasing and upscaling"): temporal AA upsamples on the
//!   desktop tiers (M1 R7); elsewhere, below render scale 1, the scene is upsampled bilinearly.
//!   A spatial upscaler (an FSR1-style EASU and RCAS port, or MetalFX spatial) takes over on
//!   phones, and DLSS or MetalFX temporal can replace the resolve ([`temporal`] module docs);
//!   the quality tiers' render scale and dynamic resolution drive their input size.
//! - **The environment shader** (see above): reflection probes and the sky cube's prefiltered
//!   mips (W5) replace the hemisphere's reflection; the wet surface model reads the frame's weather; screen-space
//!   edges for the environment (desktop).
//! - **The sky** (see the `sky` module's later milestones): weather, moon phases, cloud shadows,
//!   and the sky cube's readers (W4, W5) with its prefiltered mips.
//! - **The anime character shader** (see above) grows layered eyes (C6) on the eye permutation.
//!   Its reflection add takes W5's prefiltered reflection cube, at the region's roughness, on
//!   every tier, and screen-space reflections on the desktop tiers later; VRM's MToon emission
//!   maps onto the layers' emission; the proximity fade could give fading pixels their own
//!   responsive weight, so temporal AA settles the dither faster. Outlines take their width's region per vertex (the vertex stage reads no
//!   textures): a width texture would make group 3 visible to it. Cooked MikkTSpace tangents
//!   replace the normal map's screen-derivative frame (outline normals need none: their frame is
//!   derived from the normal), and give Kajiya-Kay a strand direction of the mesh's own.
//! - **Rim, hair and eyes through hair** (see above): a rim against the environment's depth
//!   step too (today anything that isn't a character counts as far behind), from a scene depth
//!   every tier keeps; a per-character rim scale in `CharacterData` if one material needs two;
//!   the bangs' shadow from local lights; faces through hair for transparent faces; a rim mask
//!   texture (MToon's `rimMultiplyTexture`) if per-region strength proves too coarse; under
//!   MSAA, a through-hair depth nudge from the hair depth's slope (today a fixed ratio from the
//!   pixel center's depth, which samples of bangs steeper than about 80° to the view can fail).
//! - **Anime eyes** (see above): the eye depth mask is single-sampled, so under MSAA a layer's
//!   edge against a lid steps by whole pixels (a per-sample mask, or a coverage-weighted one from
//!   a gather, if close-ups show it); through hair, each layer blends over the hair on its own, so
//!   stacked layers pale the iris a little where the eye white shows under its coverage (a
//!   composite of the layers in one draw would fix it); highlights share their instance's phase
//!   (a per-material phase offset if one eye's two highlights should glint apart); a refracted
//!   ray (an index of refraction) instead of the offset limit, if the shift should grow toward
//!   grazing angles.
//! - **Faces** (see above): light layers,
//!   so local lights split on faces by the face map or not at all (today they split on the
//!   face's normals, as on the body); a face-SDF threshold debug view (F5's views).
//! - **Shadows** (see the [`shadows`] module's later milestones): alpha-tested casters of the
//!   other families cut out (foliage's do, M1 W6), characters shadowing each other, a static far
//!   term past the cascades, cheaper receivers (a smaller far tent or a desktop shadow mask),
//!   clearing only the atlas tiles drawn, cloud shadows and local-light shadows, and GPU culling
//!   of casters.
//! - **Permutations' remaining passes:** the prepasses cut the other families' alpha-tested
//!   materials out (a variant binding group 3, as foliage's do) once something reads them;
//!   transparent surfaces sort back to front with draw sorting; alpha-to-coverage for the other
//!   families' alpha-tested surfaces under MSAA (foliage writes its own sample mask).
//! - **Anisotropic filtering** joins the quality tiers.
//! - **Shader hot reload** compiles on workers and swaps the pipelines in when they are ready,
//!   with the warm-up (spec §5.12), instead of on the frame that applies an edit; WESL imports
//!   replace the concatenation.

pub mod ambient;
mod bind_cache;
mod bounds;
mod buffer;
mod character_prepass;
mod characters;
pub mod debug_view;
mod draw;
mod environment;
pub mod fog;
pub mod foliage;
mod frame;
mod gpu_types;
pub mod graph;
mod lights;
mod materials;
mod mesh_pool;
mod outlines;
mod pipelines;
pub mod post;
pub mod quality;
mod renderer;
mod scene;
mod shaders;
pub mod shadows;
mod skinning;
mod sky;
pub mod temporal;
pub mod texture;

pub use ambient::{
    AmbientOcclusion, AmbientStats, GtaoSettings, MAX_PROBE_KEYS, ProbeKeys, ReflectionSettings,
};
pub use character_prepass::CharacterPrepassStats;
pub use debug_view::{DebugView, DebugViewStats};
pub use environment::{EnvironmentSettings, EnvironmentSpecular, ProbeLookup};
pub use fog::FogStats;
pub use foliage::FoliageSettings;
pub use frame::{
    AmbientLight, CameraData, Celestial, CharacterBody, CharacterFace, CharacterGi, CharacterInstance,
    CharacterLight, CharacterLighting, CloudLight, DirectionalLight, FaceExpression, Fog, FogLayer,
    FramePacket, FramePacketRing, HeadBasis, HeightFog, InstanceDeltas, InstanceId, LightKind, LocalLight,
    MeshInstance, PacketRange, SkinnedInstance, SkyDisc, SkyGradient, SkyLayers, ViewData, Weather, Wind,
};
pub use lights::{LightPath, LightStats, MAX_LIGHTS};
pub use materials::MaterialHandle;
pub use mesh_pool::MeshHandle;
pub use post::{
    BloomParams, CharacterBloom, ChromaticAberration, ColorGrade, Diffusion, DrawnEffects, Duotone,
    EffectCoverage, GradientBlend, GradientLayer, GradientShape, Inversion, LensFlare, MAX_BLOOM_LEVELS,
    MAX_GRADIENT_LAYERS, MAX_RADIAL_BLUR_TAPS, PostEffectSettings, PostEffects, PostParams, PostStats,
    RadialBlur, SpeedLines, ToneCurve, Vignette,
};
pub use renderer::{
    CHARACTER_PREPASS, MAIN_PASS, MIN_RENDER_SCALE, Msaa, PREPASSES, PrepassTargets, RenderError,
    RenderSettings, RenderStats, Renderer, SCENE_PREPASS, ShaderUpdate, scaled_size,
};
pub use shaders::{ShaderError, ShaderFile, ShaderSources};
pub use shadows::{ShadowFilter, ShadowSettings, ShadowStats};
pub use skinning::SkinningStats;
pub use sky::{SkySettings, SkyStats};
pub use temporal::{HistoryReset, TemporalStats};

/// Log target for this crate.
pub(crate) const LOG_TARGET: &str = "sapphire::render";
